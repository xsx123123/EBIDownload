use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose;
use base64::Engine;
use ebidownload_core::progress_store::ProgressStore;
use rand::Rng;
use std::sync::Arc;

static PROGRESS_KEY: &[u8; 32] = include_bytes!(concat!(env!("OUT_DIR"), "/progress_key.bin"));

pub fn progress_key_hex() -> String {
    hex::encode(PROGRESS_KEY)
}

struct HttpState {
    store: ProgressStore,
}

pub async fn start_progress_server(port: u16, store: ProgressStore) -> anyhow::Result<()> {
    let state = Arc::new(HttpState { store });

    let app = Router::new()
        .route("/progress", get(handle_progress))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!("Progress API server listening on 0.0.0.0:{}", port);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_progress(State(state): State<Arc<HttpState>>) -> impl IntoResponse {
    let data = state.store.read().await;
    let json = match serde_json::to_vec(&*data) {
        Ok(j) => j,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "serialization failed"})),
            );
        }
    };

    let mut rng = rand::thread_rng();
    let nonce_bytes: [u8; 12] = rng.gen();
    let nonce = Nonce::from_slice(&nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(PROGRESS_KEY).expect("valid 256-bit key");
    let ciphertext = match cipher.encrypt(nonce, json.as_ref()) {
        Ok(c) => c,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "encryption failed"})),
            );
        }
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ciphertext": general_purpose::STANDARD.encode(&ciphertext),
            "nonce": general_purpose::STANDARD.encode(&nonce_bytes),
        })),
    )
}
