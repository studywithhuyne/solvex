//! Development-only policy at the HTTP boundary; declared IDs are impersonable.

use std::collections::BTreeSet;

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

use super::{auth::parse_user_id, state::AppState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    Maintenance,
    Demo,
}

#[derive(Debug, Clone)]
pub struct RuntimePolicy {
    pub mode: RuntimeMode,
    pub admin_ids: BTreeSet<u64>,
}

#[derive(Debug, Error)]
pub enum PolicyConfigError {
    #[error("CEX_MODE must be maintenance or demo")]
    InvalidMode,
    #[error("CEX_ADMIN_USER_IDS must contain positive IDs within the SQL BIGINT range")]
    InvalidAdminId,
}

impl RuntimePolicy {
    pub fn from_config(mode: &str, admin_ids: &str) -> Result<Self, PolicyConfigError> {
        let mode = match mode {
            "maintenance" => RuntimeMode::Maintenance,
            "demo" => RuntimeMode::Demo,
            _ => return Err(PolicyConfigError::InvalidMode),
        };
        let mut ids = BTreeSet::new();
        if !admin_ids.trim().is_empty() {
            for raw in admin_ids.split(',') {
                let id = raw
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| PolicyConfigError::InvalidAdminId)?;
                if id == 0 || i64::try_from(id).is_err() {
                    return Err(PolicyConfigError::InvalidAdminId);
                }
                ids.insert(id);
            }
        }
        Ok(Self {
            mode,
            admin_ids: ids,
        })
    }

    pub fn from_env() -> Result<Self, PolicyConfigError> {
        let mode = std::env::var("CEX_MODE").unwrap_or_else(|_| "maintenance".into());
        let ids = std::env::var("CEX_ADMIN_USER_IDS").unwrap_or_default();
        Self::from_config(&mode, &ids)
    }

    pub fn allows_route(&self, method: &Method, path: &str) -> bool {
        if self.mode == RuntimeMode::Demo {
            return true;
        }
        !is_demo_route(path) && matches!(*method, Method::GET | Method::HEAD)
    }

    pub fn allows_actor(&self, method: &Method, path: &str, id: u64, suspended: bool) -> bool {
        if is_privileged_route(path) {
            return self.admin_ids.contains(&id) && !suspended;
        }
        !suspended
            || matches!(*method, Method::GET | Method::HEAD)
            || (*method == Method::DELETE && path.starts_with("/api/orders/"))
    }
}

fn is_privileged_route(path: &str) -> bool {
    path.starts_with("/api/admin/") || path.starts_with("/api/simulator/")
}

fn is_demo_route(path: &str) -> bool {
    is_privileged_route(path)
        || matches!(
            path,
            "/api/deposit" | "/api/auth/register" | "/api/auth/login" | "/api/auth/users"
        )
}

fn needs_actor(method: &Method, path: &str) -> bool {
    is_privileged_route(path)
        || path.starts_with("/api/balances")
        || path.starts_with("/api/orders")
        || matches!(
            path,
            "/api/auth/me" | "/api/auth/users" | "/api/trades/user" | "/api/zkp/proof"
        )
        || (!matches!(*method, Method::GET | Method::HEAD)
            && !matches!(path, "/api/auth/register" | "/api/auth/login"))
}

fn denied(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

pub async fn enforce_policy(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let actor = parse_user_id(request.headers()).ok();
    let response = if !state.policy.allows_route(&method, &path) {
        denied(StatusCode::FORBIDDEN, "route disabled by runtime mode")
    } else if needs_actor(&method, &path) {
        match parse_user_id(request.headers()) {
            Err(error) => error.into_response(),
            Ok(id) => match i64::try_from(id) {
                Err(_) => denied(
                    StatusCode::BAD_REQUEST,
                    "x-user-id exceeds SQL BIGINT range",
                ),
                Ok(sql_id) => {
                    // Query metadata is checked against the dedicated project test database.
                    match sqlx::query!("SELECT is_suspended FROM users WHERE id = $1", sql_id)
                        .fetch_optional(&state.db)
                        .await
                    {
                        Err(error) => {
                            tracing::error!(%error, "policy lookup failed");
                            denied(StatusCode::SERVICE_UNAVAILABLE, "policy lookup unavailable")
                        }
                        Ok(None) => denied(StatusCode::UNAUTHORIZED, "unknown declared user"),
                        Ok(Some(user))
                            if !state.policy.allows_actor(
                                &method,
                                &path,
                                id,
                                user.is_suspended,
                            ) =>
                        {
                            denied(
                                StatusCode::FORBIDDEN,
                                "actor is not allowed to perform this action",
                            )
                        }
                        Ok(Some(_)) => next.run(request).await,
                    }
                }
            },
        }
    } else {
        next.run(request).await
    };
    if is_privileged_route(&path) {
        tracing::info!(declared_actor = ?actor, action = %method, route = %path,
            status = response.status().as_u16(), "demo policy audit");
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_defaults_are_read_only_and_bad_configuration_is_rejected() {
        let policy = RuntimePolicy::from_config("maintenance", "").unwrap();
        assert!(policy.allows_route(&Method::GET, "/api/orderbook"));
        for path in [
            "/api/orders",
            "/api/withdraw",
            "/api/auth/register",
            "/api/transfer",
        ] {
            assert!(!policy.allows_route(&Method::POST, path));
        }
        assert!(!policy.allows_route(&Method::GET, "/api/admin/metrics"));
        assert!(RuntimePolicy::from_config("production", "1").is_err());
        assert!(RuntimePolicy::from_config("demo", "0").is_err());
        assert!(RuntimePolicy::from_config("demo", "18446744073709551615").is_err());
    }

    #[test]
    fn suspension_and_admin_policy_are_server_owned() {
        let policy = RuntimePolicy::from_config("demo", "2").unwrap();
        assert!(!policy.allows_actor(&Method::GET, "/api/admin/metrics", 1, false));
        assert!(policy.allows_actor(&Method::GET, "/api/admin/metrics", 2, false));
        assert!(!policy.allows_actor(&Method::GET, "/api/admin/metrics", 2, true));
        assert!(policy.allows_actor(&Method::GET, "/api/balances", 1, true));
        assert!(policy.allows_actor(&Method::DELETE, "/api/orders/4", 1, true));
        for path in [
            "/api/orders",
            "/api/deposit",
            "/api/withdraw",
            "/api/transfer",
            "/api/auth/display-name",
        ] {
            assert!(!policy.allows_actor(&Method::POST, path, 1, true));
        }
    }
}
