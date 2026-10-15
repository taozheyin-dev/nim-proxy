//! Racing control-plane boundaries through the real server and durable store.

#[allow(dead_code)]
mod support;

use reqwest::{Client, Method, Response};
use serde_json::{json, Value};
use support::*;

fn same_model() -> Value {
    json!({"type":"same_model", "attempts":2, "hedge_delay_ms":0})
}

async fn assert_error(response: Response, status: u16, code: &str) -> Value {
    assert_eq!(response.status().as_u16(), status);
    assert_eq!(response.headers()["content-type"], "application/json");
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], code);
    assert_eq!(body["error"]["type"], "proxy_error");
    body
}

#[tokio::test]
async fn racing_control_plane_requires_admin() {
    let client = Client::new();
    let fresh = start_proxy_fresh().await;
    let mock = start_mock().await;
    let proxy = start_proxy_with(
        &mock.url,
        StoreOpts {
            extra_users: vec![
                ("operator".into(), "admin".into()),
                ("reader".into(), "user".into()),
            ],
            ..StoreOpts::default()
        },
        &[],
    )
    .await;
    let reader = login_as(&proxy, "reader").await;
    let admin = login_as(&proxy, "operator").await;
    let before = std::fs::read(proxy.data_dir.join("config.json")).unwrap();
    for (method, path) in [
        (Method::GET, "/api/racing/config"),
        (Method::POST, "/api/racing/config"),
        (Method::GET, "/api/racing/stats"),
        (Method::GET, "/api/racing/ranked"),
    ] {
        let body = json!({"enabled":false});
        assert_error(
            client
                .request(method.clone(), fresh.url(path))
                .json(&body)
                .send()
                .await
                .unwrap(),
            503,
            "setup_required",
        )
        .await;
        assert_error(
            client
                .request(method.clone(), proxy.url(path))
                .json(&body)
                .send()
                .await
                .unwrap(),
            401,
            "unauthorized",
        )
        .await;
        assert_error(
            client
                .request(method.clone(), proxy.url(path))
                .header("cookie", &reader)
                .json(&body)
                .send()
                .await
                .unwrap(),
            403,
            "forbidden",
        )
        .await;
        assert_eq!(
            std::fs::read(proxy.data_dir.join("config.json")).unwrap(),
            before,
            "rejected {path} must preserve durable bytes"
        );
        // An admin may read every racing surface. Mutations are exercised below.
        if method == Method::GET {
            let response = client
                .get(proxy.url(path))
                .header("cookie", &admin)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-type"], "application/json");
        }
    }
    for body in ["{", "{}"] {
        assert_error(
            client
                .post(proxy.url("/api/racing/config"))
                .header("cookie", &reader)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await
                .unwrap(),
            403,
            "forbidden",
        )
        .await;
    }
    assert_error(
        client
            .post(proxy.url("/api/racing/config"))
            .header("cookie", &reader)
            .body("not JSON")
            .send()
            .await
            .unwrap(),
        403,
        "forbidden",
    )
    .await;
    assert_eq!(
        std::fs::read(proxy.data_dir.join("config.json")).unwrap(),
        before
    );
    assert_eq!(
        mock.state.hit_count(),
        0,
        "management requests never contact NIM"
    );
}

#[tokio::test]
async fn racing_configuration_is_validated_atomic_and_durable() {
    let mock = start_mock().await;
    let mut proxy = start_proxy(&mock.url, &[]).await;
    let client = Client::new();
    let mut cookie = login(&proxy).await;
    let config_path = proxy.data_dir.join("config.json");
    let initial = client
        .get(proxy.url("/api/racing/config"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(initial, "{\"enabled\":false,\"strategy\":null}");
    assert!(
        serde_json::from_slice::<Value>(&std::fs::read(&config_path).unwrap())
            .unwrap()
            .get("racing")
            .is_none()
    );

    let invalid = vec![
        json!({"enabled":true}),
        json!({"enabled":true,"strategy":null}),
        json!({"enabled":true,"strategy":{"type":"none"}}),
        json!({"enabled":true,"strategy":{"type":"same_model","attempts":0,"hedge_delay_ms":0}}),
        json!({"enabled":true,"strategy":{"type":"same_model","attempts":6,"hedge_delay_ms":0}}),
        json!({"enabled":true,"strategy":{"type":"same_model","attempts":2,"hedge_delay_ms":300001}}),
    ];
    for request in invalid {
        let before = std::fs::read(&config_path).unwrap();
        assert_error(
            client
                .post(proxy.url("/api/racing/config"))
                .header("cookie", &cookie)
                .json(&request)
                .send()
                .await
                .unwrap(),
            400,
            "invalid_config",
        )
        .await;
        assert_eq!(
            std::fs::read(&config_path).unwrap(),
            before,
            "invalid racing config must preserve durable bytes: {request}"
        );
        let actual = client
            .get(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(
            actual, initial,
            "invalid racing config must preserve runtime state"
        );
    }
    for (body, content_type, status, code) in [
        ("{", Some("application/json"), 400, "invalid_json"),
        ("{}", Some("application/json"), 422, "invalid_json"),
        ("{}", None, 415, "unsupported_media_type"),
    ] {
        let before = std::fs::read(&config_path).unwrap();
        let mut request = client
            .post(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .body(body);
        if let Some(value) = content_type {
            request = request.header("content-type", value);
        }
        assert_error(request.send().await.unwrap(), status, code).await;
        assert_eq!(std::fs::read(&config_path).unwrap(), before);
    }
    for strategy in [
        same_model(),
        json!({"type":"same_model","attempts":1,"hedge_delay_ms":0}),
        json!({"type":"same_model","attempts":5,"hedge_delay_ms":300000}),
    ] {
        let response = client
            .post(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .json(&json!({"enabled":true,"strategy":strategy}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(response.text().await.unwrap(), "{\"ok\":true}");
        let stored: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        assert_eq!(stored["racing"], strategy);
        proxy = restart(proxy, &[]).await;
        cookie = login(&proxy).await;
        let actual: Value = client
            .get(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(actual, json!({"enabled":true,"strategy":strategy}));
    }
    let response = client
        .post(proxy.url("/api/racing/config"))
        .header("cookie", &cookie)
        .json(&json!({"enabled":false}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let stored: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    assert!(
        stored.get("racing").is_none(),
        "disabled legacy store shape is preserved"
    );
    proxy = restart(proxy, &[]).await;
    cookie = login(&proxy).await;
    let actual = client
        .get(proxy.url("/api/racing/config"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(actual, initial);
    assert_eq!(mock.state.hit_count(), 0);
}

#[tokio::test]
async fn legacy_racing_policies_require_explicit_migration_without_mutation() {
    let mock = start_mock().await;
    let proxy = start_proxy(&mock.url, &[]).await;
    let client = Client::new();
    let cookie = login(&proxy).await;
    let before = std::fs::read(proxy.data_dir.join("config.json")).unwrap();
    for strategy in [
        json!({"type":"full_parallel","models":["a","b"]}),
        json!({"type":"smart","primary_model":"a","fallback_models":["b"],"primary_timeout_ms":1}),
        json!({"type":"ttft_racing","models":["a","b"]}),
    ] {
        let body = assert_error(
            client
                .post(proxy.url("/api/racing/config"))
                .header("cookie", &cookie)
                .json(&json!({"enabled":true,"strategy":strategy}))
                .send()
                .await
                .unwrap(),
            400,
            "invalid_config",
        )
        .await;
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.contains("same_model"), "migration target: {body}");
        assert!(message.contains("attempts"), "migration action: {body}");
        assert!(
            message.contains("hedge_delay_ms"),
            "migration launch policy: {body}"
        );
        assert_eq!(
            std::fs::read(proxy.data_dir.join("config.json")).unwrap(),
            before,
            "legacy policy must not be silently rewritten: {strategy}"
        );
        let actual: Value = client
            .get(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(actual, json!({"enabled":false,"strategy":null}));
    }
    assert_eq!(mock.state.hit_count(), 0);
}

#[tokio::test]
async fn malformed_racing_strategy_fields_are_rejected_without_mutation() {
    let mock = start_mock().await;
    let proxy = start_proxy(&mock.url, &[]).await;
    let client = Client::new();
    let cookie = login(&proxy).await;
    let before = std::fs::read(proxy.data_dir.join("config.json")).unwrap();
    for strategy in [
        json!({"type":"full_parallel","models":["a"],"attempts":2,"hedge_delay_ms":0}),
        json!({"type":"same_model","attempts":2,"hedge_delay_ms":0,"models":["a"]}),
        json!({"type":"same_model","attempts":2,"hedge_delay_ms":0,"unknown":true}),
        json!({"type":"same_model","attempts":2}),
        json!({"type":"same_model","hedge_delay_ms":0}),
        json!({"type":"same_model","attempts":-1,"hedge_delay_ms":0}),
        json!({"type":"same_model","attempts":1.5,"hedge_delay_ms":0}),
        json!({"type":"same_model","attempts":2,"hedge_delay_ms":-1}),
        json!({"type":"same_model","attempts":2,"hedge_delay_ms":0.5}),
        json!({"type":"unknown","attempts":2,"hedge_delay_ms":0}),
    ] {
        assert_error(
            client
                .post(proxy.url("/api/racing/config"))
                .header("cookie", &cookie)
                .json(&json!({"enabled":true,"strategy":strategy}))
                .send()
                .await
                .unwrap(),
            422,
            "invalid_json",
        )
        .await;
        assert_eq!(
            std::fs::read(proxy.data_dir.join("config.json")).unwrap(),
            before,
            "malformed policy changed durable bytes: {strategy}"
        );
        let actual: Value = client
            .get(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(actual, json!({"enabled":false,"strategy":null}));
    }
    assert_eq!(mock.state.hit_count(), 0);
}

#[tokio::test]
async fn corrupt_racing_store_refuses_startup_without_rewriting_bytes() {
    for strategy in [
        json!({"type":"same_model","attempts":0,"hedge_delay_ms":0}),
        json!({"type":"same_model","attempts":6,"hedge_delay_ms":0}),
        json!({"type":"same_model","attempts":2,"hedge_delay_ms":300001}),
        json!({"type":"same_model","attempts":2,"hedge_delay_ms":0,"models":["a"]}),
        json!({"type":"full_parallel","models":["a"]}),
        json!({"type":"smart","primary_model":"a","fallback_models":[],"primary_timeout_ms":1}),
        json!({"type":"ttft_racing","models":["a"]}),
        json!({"type":"none"}),
        json!({"type":"unknown"}),
    ] {
        let dir = scratch_data_dir();
        let mut store = StoreOpts::default().json("http://127.0.0.1:9");
        store["racing"] = strategy;
        std::fs::write(dir.join("config.json"), serde_json::to_vec(&store).unwrap()).unwrap();
        expect_refuses_to_start(dir).await;
    }
}

#[tokio::test]
async fn racing_and_strict_passthrough_are_mutually_exclusive() {
    let mock = start_mock().await;
    let proxy = start_proxy_with(
        &mock.url,
        StoreOpts {
            strict_passthrough: true,
            ..StoreOpts::default()
        },
        &[],
    )
    .await;
    let client = Client::new();
    let cookie = login(&proxy).await;
    let path = proxy.data_dir.join("config.json");
    let before = std::fs::read(&path).unwrap();
    let enable = json!({"enabled":true, "strategy":same_model()});
    assert_error(
        client
            .post(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .json(&enable)
            .send()
            .await
            .unwrap(),
        400,
        "invalid_config",
    )
    .await;
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let mut limits = json!({"max_wait_secs":30,"heartbeat_secs":1,"models_ttl_secs":600,"stream_idle_secs":300,"request_timeout_secs":300,"max_inflight":512,"strict_passthrough":false});
    assert_eq!(
        client
            .post(proxy.url("/api/settings/limits"))
            .header("cookie", &cookie)
            .json(&limits)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .post(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .json(&enable)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let before = std::fs::read(&path).unwrap();
    limits["strict_passthrough"] = json!(true);
    let runtime_before: Value = client
        .get(proxy.url("/api/config"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    for endpoint in ["/api/settings/limits", "/api/settings/server"] {
        let mut request = limits.clone();
        if endpoint == "/api/settings/server" {
            // A conflicting complete save must also roll back its upstream edit.
            request["base_url"] = json!("http://127.0.0.1:9");
        }
        assert_error(
            client
                .post(proxy.url(endpoint))
                .header("cookie", &cookie)
                .json(&request)
                .send()
                .await
                .unwrap(),
            400,
            "invalid_config",
        )
        .await;
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "rejected {endpoint} must preserve durable bytes"
        );
        let runtime_after: Value = client
            .get(proxy.url("/api/config"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            runtime_after, runtime_before,
            "rejected {endpoint} must preserve runtime settings"
        );
        let racing: Value = client
            .get(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(racing, enable);
    }
    let dir = scratch_data_dir();
    let mut stored: Value = serde_json::from_slice(&before).unwrap();
    stored["limits"]["strict_passthrough"] = json!(true);
    std::fs::write(
        dir.join("config.json"),
        serde_json::to_vec(&stored).unwrap(),
    )
    .unwrap();
    expect_refuses_to_start(dir).await;
    assert_eq!(mock.state.hit_count(), 0);
}

#[tokio::test]
async fn racing_save_failure_keeps_store_and_runtime() {
    let mock = start_mock().await;
    let proxy = start_proxy(&mock.url, &[]).await;
    let cookie = login(&proxy).await;
    let client = Client::new();
    let before = std::fs::read(proxy.data_dir.join("config.json")).unwrap();
    // Occupying the writer's temporary filename with a directory induces a
    // real I/O failure without changing the valid committed store.
    std::fs::create_dir(proxy.data_dir.join("config.json.tmp")).unwrap();
    let body = assert_error(
        client
            .post(proxy.url("/api/racing/config"))
            .header("cookie", &cookie)
            .json(&json!({"enabled":true,"strategy":same_model()}))
            .send()
            .await
            .unwrap(),
        400,
        "invalid_config",
    )
    .await;
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot write the config store"),
        "the injected filesystem failure must reach the save boundary: {body}"
    );
    assert_eq!(
        std::fs::read(proxy.data_dir.join("config.json")).unwrap(),
        before
    );
    let actual = client
        .get(proxy.url("/api/racing/config"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(actual, "{\"enabled\":false,\"strategy\":null}");
    assert_eq!(mock.state.hit_count(), 0);
}
