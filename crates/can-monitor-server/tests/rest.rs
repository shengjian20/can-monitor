//! # 集成测试: REST API
//!
//! 用 axum 测试客户端 (`tower::ServiceExt::oneshot` + `http-body-util`) 直接
//! 驱动 [`router`] 构造的应用, 不真正监听端口:
//!
//! 1. `GET /api/devices` → 200 + JSON 数组 (本机无 CAN 设备时为空数组);
//! 2. `POST /api/send` `write_enabled=false` → **403** (写门控);
//! 3. `POST /api/send` `write_enabled=true` → 200 (帧进入总线发送队列);
//! 4. `GET /api/status` → 200 + `running` 字段 (默认 false);
//! 5. `POST /api/monitor/start` / `stop` → 200 且开关状态联动。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use can_monitor_core::bus::MonitorBus;
use can_monitor_server::router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

/// 构造一个未启动 reader 的共享总线 (仅供 REST 门控 / 状态断言)。
fn make_bus() -> Arc<MonitorBus> {
    let (bus, _rx, _err_rx) = MonitorBus::new();
    Arc::new(bus)
}

/// 发一次 JSON POST 请求, 返回响应 (不消费 body)。
async fn post_json(app: axum::Router, uri: &str, body: &str) -> axum::http::Response<Body> {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
    .unwrap()
}

/// 读取响应 body 并解析为 JSON。
async fn body_json(resp: axum::http::Response<Body>) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// `GET /api/devices`: 200 + JSON 数组 (结构字段与 Tauri list_devices 一致)。
#[tokio::test]
async fn devices_returns_json_array() {
    let app = router(make_bus(), false);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    let devices = json.as_array().expect("devices 应为 JSON 数组");
    for d in devices {
        // 契约字段齐全。
        for key in ["id", "name", "kind", "driver", "model"] {
            assert!(d.get(key).is_some(), "设备条目应含字段 {key}: {d}");
        }
        assert!(d.get("available").is_some());
    }
}

/// `POST /api/send` 未启用写模式 → **403** (写门控, 与请求内容无关)。
#[tokio::test]
async fn send_without_write_enabled_is_forbidden() {
    let app = router(make_bus(), false);
    let resp = post_json(
        app,
        "/api/send",
        r#"{"id":"123","ext":false,"data":"01 02 03"}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "写未启用应 403");
    let json = body_json(resp).await;
    assert!(
        json.get("error").is_some(),
        "403 响应应带 error 说明: {json}"
    );
}

/// `POST /api/send` 启用写模式 → 200 (帧进总线发送队列; 无后端时队列暂存)。
#[tokio::test]
async fn send_with_write_enabled_succeeds() {
    let bus = make_bus();
    let app = router(Arc::clone(&bus), true);
    let resp = post_json(
        app,
        "/api/send",
        r#"{"id":"0x123","ext":false,"data":"01 02 03"}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "写启用后发送应 200");
    let json = body_json(resp).await;
    assert_eq!(json["ok"], Value::Bool(true));
}

/// `POST /api/send` 帧格式非法 → 400 (仅写启用时, 门控已通过后解析失败)。
#[tokio::test]
async fn send_bad_frame_is_bad_request() {
    let app = router(make_bus(), true);
    let resp = post_json(app, "/api/send", r#"{"id":"zzz","ext":false,"data":"GG"}"#).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "非法帧应 400");
}

/// `POST /api/send` 标准帧 ID 超出 11 位范围 → 400。
///
/// 回归 P1 (F2): 过去 `CanId::new_standard(raw_id as u16)` 先截断再校验,
/// 0x1FFF0000 / 0x10000 这类低 16 位落入合法区间的值会被静默截断接受,
/// 现在必须先查界再转型, 一律拒绝。
#[tokio::test]
async fn send_standard_id_out_of_range_is_bad_request() {
    let app = router(make_bus(), true);
    for id in ["0x1FFF0000", "0x10000", "0x107FF", "0x800"] {
        let body = format!(r#"{{"id":"{id}","ext":false,"data":"01 02 03"}}"#);
        let resp = post_json(app.clone(), "/api/send", &body).await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "标准帧 ID {id} 超出 0x7FF 应 400 (不得静默截断)"
        );
        let json = body_json(resp).await;
        assert!(
            json.get("error").is_some(),
            "400 响应应带中文 error 说明: {json}"
        );
    }
}

/// `POST /api/send` 合法标准帧 ID (≤0x7FF) 仍接受。
#[tokio::test]
async fn send_valid_standard_id_succeeds() {
    let app = router(make_bus(), true);
    let resp = post_json(
        app,
        "/api/send",
        r#"{"id":"0x181","ext":false,"data":"01 02 03"}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "合法标准帧 ID 0x181 应 200");
}

/// `POST /api/send` 合法扩展帧 ID (≤0x1FFFFFFF) 仍接受, 行为不变。
#[tokio::test]
async fn send_valid_extended_id_succeeds() {
    let app = router(make_bus(), true);
    let resp = post_json(
        app,
        "/api/send",
        r#"{"id":"0x18FF1234","ext":true,"data":"01 02 03"}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "合法扩展帧 ID 应 200");
}

/// `GET /api/status`: 200 + running 字段 (默认关闭) + 计数器字段齐全。
#[tokio::test]
async fn status_reports_running_and_counts() {
    let app = router(make_bus(), false);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_json(resp).await;
    assert_eq!(json["running"], Value::Bool(false), "默认监控关闭");
    for key in ["total", "canopen", "j1939", "error", "dropped"] {
        assert!(json.get(key).is_some(), "status 应含字段 {key}: {json}");
    }
}

/// `POST /api/monitor/start` / `stop`: 开关状态随请求联动 (只读控制, 恒可用)。
#[tokio::test]
async fn monitor_start_stop_toggles_running() {
    let bus = make_bus();
    let app = router(Arc::clone(&bus), false);

    // 初始关闭。
    assert!(!bus.is_monitoring());

    // start (合法设备 ID) → 开启。
    let resp = post_json(
        app.clone(),
        "/api/monitor/start",
        r#"{"device_id":"socketcan:can0"}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(bus.is_monitoring(), "start 后应开启监控");

    // stop → 关闭。
    let resp = post_json(app.clone(), "/api/monitor/stop", "{}").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(!bus.is_monitoring(), "stop 后应关闭监控");

    // start 非法 device_id → 400, 开关不受影响。
    let resp = post_json(app, "/api/monitor/start", r#"{"device_id":"bogus:1"}"#).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(!bus.is_monitoring());
}
