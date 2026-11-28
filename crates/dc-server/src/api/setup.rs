//! The setup wizard's connection check (DECISIONS E25). The account itself
//! is created by `POST /api/setup` in `auth.rs`.

use super::auth::verify_setup_code;
use crate::AppState;
use crate::http::{ClientIp, Failure, JsonBody};
use axum::Json;
use axum::extract::State;
use dc_core::setup::{ConnectionCheck, SetupCheckRequest, SetupCheckResponse};

/// `POST /api/setup/check`: needs the one-time setup code, under the same
/// limits and backoff as `POST /api/setup`. Reports whether the practice
/// records connector answers and whether a text provider is configured.
/// Nothing is stored, and no patient data leaves the server.
pub async fn check(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    JsonBody(request): JsonBody<SetupCheckRequest>,
) -> Result<Json<SetupCheckResponse>, Failure> {
    verify_setup_code(&state, ip, &request.setup_code).await?;

    let connector = state.pms.name().to_owned();
    let practice_records = match state.pms.recommended_patients().await {
        Ok(_) => ConnectionCheck {
            ok: true,
            label: connector,
            message: "Connected. DentalClarity can read the practice's treatment plans.".into(),
        },
        Err(_) => {
            // The error text can echo database details, so it is not logged.
            tracing::warn!("setup check: practice records connector failed");
            ConnectionCheck {
                ok: false,
                label: connector,
                message: "Could not read the practice records. Check the connection settings in the server configuration, then run the check again.".into(),
            }
        }
    };
    let ai_provider = match &state.generation {
        Some(_) => ConnectionCheck {
            ok: true,
            label: "Configured".into(),
            message: "A text provider is configured for writing presentations.".into(),
        },
        None => ConnectionCheck {
            ok: false,
            label: "Not configured".into(),
            message: "No text provider is configured yet, so presentations cannot be written. You can finish setup now and add the provider in the server configuration later.".into(),
        },
    };
    Ok(Json(SetupCheckResponse {
        practice_records,
        ai_provider,
    }))
}
