use crate::domain::card_number::CardNumber;
use crate::html_gen::{SubmitCardNumberPage, SubmitPaymentPage};
use crate::startup::AppState;
use acquisim_api::init_payment::PaymentOperationNotification;
use acquisim_api::register_card_token::RegisterCardTokenOperationResult;
use askama::Template;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::{routing, Json, Router};
use secrecy::Secret;
use serde::Deserialize;
use uuid::Uuid;

// ───── Types ────────────────────────────────────────────────────────────── //

#[derive(Debug, Deserialize)]
pub struct Credentials {
    card_number: CardNumber,
    password: Secret<String>,
}

// ───── Handlers ─────────────────────────────────────────────────────────── //

pub fn html_pages_and_triggers_router() -> Router<AppState> {
    Router::new()
        .route("/payment_page/:id", routing::get(payment_html_page))
        .route("/payment/:id", routing::post(trigger_payment))
        .route(
            "/register_card_token_page/:id",
            routing::get(card_token_registration_html_page),
        )
        .route(
            "/card_token/:id",
            routing::get(trigger_card_token_registration),
        )
}

#[tracing::instrument(name = "Get payment html page", skip_all)]
pub async fn payment_html_page(
    State(state): State<AppState>,
    Path(payment_id): Path<Uuid>,
) -> Result<Html<String>, StatusCode> {
    // Try to create submit payment url for client (browser)
    let submit_payment_url = match format!(
        "http://{}:{}/payment/{}",
        state.settings.addr, state.settings.port, payment_id
    )
    .parse()
    {
        Ok(url) => url,
        Err(e) => {
            tracing::error!("Failed to parse string as url: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Try to return html payment page
    match state
        .interaction_sessions
        .try_acquire_session_by_id(payment_id)
    {
        Ok(entity) => {
            let req = entity.session_type.payment_req();
            match SubmitPaymentPage::new(req.amount, submit_payment_url)
                .render()
            {
                Ok(body) => Ok(Html(body)),
                Err(e) => {
                    tracing::error!("Failed to render payment html page: {e}");
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to get payment html page: {e}");
            Err(StatusCode::NOT_FOUND)
        }
    }
}

/// We return `String` with redirection url.
#[tracing::instrument(name = "Trigger payment", skip_all)]
pub async fn trigger_payment(
    State(state): State<AppState>,
    Path(payment_id): Path<Uuid>,
    Json(creds): Json<Credentials>,
) -> Result<String, StatusCode> {
    let session = match state
        .interaction_sessions
        .try_acquire_session_by_id(payment_id)
    {
        Ok(session) => session,
        Err(e) => {
            // No such payment
            tracing::error!("Payment not found: {e}");
            return Err(StatusCode::BAD_REQUEST);
        }
    };
    let req = session.session_type.payment_req();

    // Authorize card and password
    let account = match state
        .bank
        .authorize_account(&creds.card_number, &creds.password)
        .await
    {
        Ok(acc) => acc,
        Err(e) => {
            // Not authorized
            tracing::error!("Can't authorize account: {e}");
            return Ok(req.fail_url.to_string());
        }
    };

    // Check store account
    let store_account = state.bank.get_store_account().await;
    if !store_account.card().eq(&session.store_card) {
        tracing::error!("Failed to perform payment: wrong store account!");
        return Ok(req.fail_url.to_string());
    }

    // Perform transaction
    let result = match state
        .bank
        .new_transaction(&account, &store_account, req.amount)
        .await
    {
        Ok(()) => {
            let notification = PaymentOperationNotification::success();
            state
                .interaction_sessions
                .notify_and_remove(session.id(), &notification)
                .await;
            Ok(req.success_url.to_string())
        }
        Err(e) => {
            tracing::error!("Transaction failed: {e}");
            Ok(req.fail_url.to_string())
        }
    };

    result
}

#[tracing::instrument(name = "Get card token registration html page", skip_all)]
pub async fn card_token_registration_html_page(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Html<String>, StatusCode> {
    // Try to create submit payment url for client (browser)
    let submit_payment_url = match format!(
        "http://{}:{}/card_token/{}",
        state.settings.addr, state.settings.port, id
    )
    .parse()
    {
        Ok(url) => url,
        Err(e) => {
            tracing::error!("Failed to parse string as url: {e}");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Try to return html for card token registration
    match state.interaction_sessions.try_acquire_session_by_id(id) {
        Ok(session) => {
            let _ = session.session_type.register_card_token_req();
            match SubmitCardNumberPage::new(submit_payment_url).render() {
                Ok(body) => Ok(Html(body)),
                Err(e) => {
                    tracing::error!("Failed to render payment html page: {e}");
                    Err(StatusCode::INTERNAL_SERVER_ERROR)
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to get payment html page: {e}");
            Err(StatusCode::NOT_FOUND)
        }
    }
}

/// We return `String` with redirection url.
#[tracing::instrument(name = "Trigger card token registration", skip_all)]
pub async fn trigger_card_token_registration(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    card_number: String,
) -> Result<String, StatusCode> {
    let session = match state.interaction_sessions.try_acquire_session_by_id(id)
    {
        Ok(session) => session,
        Err(e) => {
            // No such payment
            tracing::error!("Request with id {id} not found: {e}");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    let req = session.session_type.register_card_token_req();

    // Authorize card and password
    let Ok(card) = CardNumber::parse(&card_number) else {
        tracing::error!(
            "Bad request, can't parse card number: {}",
            card_number
        );
        return Ok(req.fail_url.to_string());
    };

    // Check store account
    let store_account = state.bank.get_store_account().await;
    if !store_account.card().eq(&session.store_card) {
        tracing::error!("Failed to register card token: wrong store account!");
        return Ok(req.fail_url.to_string());
    }

    // Generate token
    let token = match state.bank.new_card_token(card).await {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("Failed to generate new card token: {e}");
            return Ok(req.fail_url.to_string());
        }
    };

    let notification = RegisterCardTokenOperationResult::success(token);
    // Notify on success registration and remove request from active list
    state
        .interaction_sessions
        .notify_and_remove(session.id(), &notification)
        .await;

    Ok(req.success_url.to_string())
}