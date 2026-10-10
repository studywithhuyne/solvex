use matching_engine::api::{policy::RuntimePolicy, router::build_router, state::AppState};
use matching_engine::db::worker::PersistenceEvent;
use matching_engine::observability::metrics::install_prometheus_recorder;
use reqwest::{Client, StatusCode};
use sqlx::PgPool;
use tokio::sync::mpsc;

/// Explicitly requires a loopback-only, project-owned test database.
#[tokio::test]
#[ignore = "requires CEX_TEST_DATABASE_URL for dedicated cex_test PostgreSQL"]
async fn policy_is_enforced_by_the_real_http_router() {
    let url = std::env::var("CEX_TEST_DATABASE_URL").expect("set dedicated test database URL");
    assert!(url.ends_with("/cex_test"));
    assert!(url.contains("@127.0.0.1:") || url.contains("@localhost:"));
    let db = PgPool::connect(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&db).await.unwrap();
    sqlx::query!(
        "INSERT INTO users (id, username, display_name, is_suspended) VALUES
        (900001, 'policy_admin', 'policy_admin', FALSE),
        (900002, 'policy_user', 'policy_user', FALSE),
        (900003, 'policy_suspended', 'policy_suspended', TRUE)
        ON CONFLICT (id) DO UPDATE SET is_suspended = EXCLUDED.is_suspended"
    )
    .execute(&db)
    .await
    .unwrap();
    let (events, _receiver) = mpsc::channel::<PersistenceEvent>(16);
    let metrics = install_prometheus_recorder().unwrap();
    let mut state = AppState::new(
        db.clone(),
        events,
        metrics,
        RuntimePolicy::from_config("demo", "900001").unwrap(),
    )
    .await
    .unwrap();
    assert!(!state.simulator.lock().running);
    let client = Client::new();

    for mode in ["demo", "maintenance"] {
        state.policy = RuntimePolicy::from_config(mode, "900001").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = build_router(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let health = client.get(format!("{base}/health")).send().await.unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        for path in [
            "/api/orders",
            "/api/deposit",
            "/api/withdraw",
            "/api/transfer",
        ] {
            let response = client
                .post(format!("{base}{path}"))
                .header("x-user-id", "900003")
                .json(&serde_json::json!({}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{mode} {path}");
        }
        let response = client
            .get(format!("{base}/api/auth/me"))
            .header("x-user-id", "900003")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = client
            .delete(format!("{base}/api/orders/999999"))
            .header("x-user-id", "900003")
            .send()
            .await
            .unwrap();
        if mode == "demo" {
            assert_ne!(response.status(), StatusCode::FORBIDDEN);
        } else {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        for actor in [
            None,
            Some("0"),
            Some("18446744073709551615"),
            Some("900099"),
        ] {
            let mut request = client.get(format!("{base}/api/auth/me"));
            if let Some(actor) = actor {
                request = request.header("x-user-id", actor);
            }
            assert!(request.send().await.unwrap().status().is_client_error());
        }
        let response = client
            .get(format!("{base}/api/admin/metrics"))
            .header("x-user-id", "900002")
            .header("Cookie", "admin_token=true")
            .header("X-Forwarded-For", "127.0.0.1")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = client
            .get(format!("{base}/api/admin/metrics"))
            .header("x-user-id", "900001")
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if mode == "demo" {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        let response = client
            .post(format!("{base}/api/auth/register"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap();
        if mode == "maintenance" {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let response = client
            .put(format!("{base}/api/auth/display-name"))
            .header("x-user-id", "900003")
            .json(&serde_json::json!({"display_name":"blocked"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        if mode == "demo" {
            let response = client
                .put(format!("{base}/api/admin/users/900001/suspend"))
                .header("x-user-id", "900001")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let response = client
                .get(format!("{base}/api/admin/metrics"))
                .header("x-user-id", "900001")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        server.abort();
        let _ = server.await;
    }
}
