use actix::Addr;
use actix_web::web::{Data, Json};
use actix_web::Responder;
use awc::http::StatusCode;
use serde_json::{json, Value};

use crate::supervisor::{GetStatusAll, HealthState, Lifecycle};
use crate::Supervisor;

pub async fn status(supervisor: Data<Addr<Supervisor>>) -> impl Responder {
    match supervisor.send(GetStatusAll).await {
        Ok(Ok(status)) => {
            let status: Value = status
                .into_iter()
                .map(|(name, lifecycle)| (name, format!("{:?}", lifecycle)))
                .collect();
            (Json(status), StatusCode::OK)
        }
        Err(e) | Ok(Err(e)) => (
            Json(json!({"error": e.to_string()})),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

pub async fn health(supervisor: Data<Addr<Supervisor>>) -> impl Responder {
    match supervisor.send(GetStatusAll).await {
        Ok(Ok(status)) => {
            let bad: Vec<_> = status
                .into_iter()
                .filter(|(_, lifecycle)| {
                    !matches!(lifecycle, Lifecycle::Running(HealthState::Healthy))
                })
                .map(|(name, _)| name)
                .collect();
            if bad.is_empty() {
                (Json(json!({"healthy": true})), StatusCode::OK)
            } else {
                (
                    Json(json!({"healthy": false, "bad_services": bad})),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            }
        }
        Err(e) | Ok(Err(e)) => (
            Json(json!({"healthy": false, "error": e.to_string()})),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}
