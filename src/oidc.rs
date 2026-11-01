//! Optional per-organisation OpenID Connect sign-in (FR-018, R8).
//!
//! Local email-and-password sign-in is never replaced or redirected away from.
//! This module owns protocol work only: discovery, the authorization-code flow
//! with PKCE, token exchange, and ID-token validation. Persistence and the
//! verified-email matching rule live in the store and the API handlers.
//!
//! Secrets are never logged: the client secret, authorization code, PKCE
//! verifier, nonce, and ID token stay out of every `Debug`/`Display` path we
//! write here, and provider errors are reduced to a short summary.

use crate::models::{OidcConnection, OidcLoginFlow};
use openidconnect::{
    core::{CoreClient, CoreProviderMetadata, CoreResponseType},
    reqwest::ClientBuilder,
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
};
use std::time::Duration;

/// Contract: discovery is attempted with a five-second timeout.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(5);

/// An in-progress sign-in that the caller must persist and present at the
/// callback. The verifier and nonce are secret and are stored, never logged.
pub struct StartedFlow {
    pub authorize_url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

/// What the provider asserted about the identity. `email_verified` is reported
/// separately so the callback can deny with the fixed `email_unverified` code.
pub struct Identity {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

fn http_client() -> Result<openidconnect::reqwest::Client, String> {
    ClientBuilder::new()
        .timeout(PROVIDER_TIMEOUT)
        // An IdP must never bounce us through a redirect chain we do not control.
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("could not create the provider client: {error}"))
}

/// Fetch and validate the provider's discovery document. Used at save time so an
/// unreachable or malformed issuer fails fast, and again during the flow so a
/// removed provider fails closed.
pub async fn discover(issuer: &str) -> Result<CoreProviderMetadata, String> {
    let issuer_url = IssuerUrl::new(issuer.to_string())
        .map_err(|error| format!("invalid issuer URL: {error}"))?;
    let http = http_client()?;
    CoreProviderMetadata::discover_async(issuer_url, &http)
        .await
        .map_err(|error| error.to_string())
}

fn redirect_url(self_url: &str) -> Result<RedirectUrl, String> {
    RedirectUrl::new(format!(
        "{}/api/oidc/callback",
        self_url.trim_end_matches('/')
    ))
    .map_err(|error| format!("invalid redirect URL: {error}"))
}

/// Build the authorization redirect. `state`, `nonce`, and the PKCE verifier are
/// returned so the caller can persist them before the browser leaves.
pub async fn start(connection: &OidcConnection, self_url: &str) -> Result<StartedFlow, String> {
    let metadata = discover(&connection.issuer).await?;
    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(connection.client_id.clone()),
        Some(ClientSecret::new(connection.client_secret.clone())),
    )
    .set_redirect_uri(redirect_url(self_url)?);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();

    let mut request = client.authorize_url(
        AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
        CsrfToken::new_random,
        Nonce::new_random,
    );
    // `openid` is added by the client itself; only the additional scopes are
    // requested here so the URL never carries a duplicate.
    for scope in connection
        .scopes
        .split_whitespace()
        .filter(|scope| *scope != "openid")
    {
        request = request.add_scope(Scope::new(scope.to_string()));
    }
    let (url, state, nonce) = request.set_pkce_challenge(challenge).url();

    Ok(StartedFlow {
        authorize_url: url.to_string(),
        state: state.secret().clone(),
        nonce: nonce.secret().clone(),
        pkce_verifier: verifier.secret().clone(),
    })
}

/// Exchange the code and verify the ID token's signature, issuer, audience,
/// expiry, and nonce. Any failure is a provider error; no partial identity is
/// returned.
pub async fn finish(
    connection: &OidcConnection,
    self_url: &str,
    flow: &OidcLoginFlow,
    code: &str,
) -> Result<Identity, String> {
    let metadata = discover(&connection.issuer).await?;
    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(connection.client_id.clone()),
        Some(ClientSecret::new(connection.client_secret.clone())),
    )
    .set_redirect_uri(redirect_url(self_url)?);
    let http = http_client()?;

    let token_request = client
        .exchange_code(AuthorizationCode::new(code.to_string()))
        .map_err(|error| format!("token endpoint unavailable: {error}"))?;
    let token_response = token_request
        .set_pkce_verifier(PkceCodeVerifier::new(flow.pkce_verifier.clone()))
        .request_async(&http)
        .await
        .map_err(|error| format!("token exchange failed: {error}"))?;

    let id_token = token_response
        .extra_fields()
        .id_token()
        .ok_or_else(|| "the provider returned no ID token".to_string())?;
    let verifier = client.id_token_verifier();
    let claims = id_token
        .claims(&verifier, &Nonce::new(flow.nonce.clone()))
        .map_err(|error| format!("ID token validation failed: {error}"))?;

    Ok(Identity {
        subject: claims.subject().to_string(),
        email: claims.email().map(|email| email.to_string()),
        email_verified: claims.email_verified().unwrap_or(false),
    })
}

/// Whether a value is an acceptable issuer: an absolute `http(s)` URL with a
/// host. Checked before discovery so a nonsense value gets an actionable error.
pub fn valid_issuer(issuer: &str) -> bool {
    if !(issuer.starts_with("https://") || issuer.starts_with("http://")) {
        return false;
    }
    IssuerUrl::new(issuer.to_string()).is_ok()
}

/// Normalise and validate the requested scopes. `openid` and `email` are
/// mandatory because matching uses the verified email claim.
pub fn normalise_scopes(scopes: Option<&str>) -> Result<String, String> {
    let raw = scopes.map(str::trim).filter(|value| !value.is_empty());
    let mut parts: Vec<String> = raw
        .unwrap_or("openid email profile")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    for required in ["openid", "email"] {
        if !parts.iter().any(|scope| scope == required) {
            parts.push(required.to_string());
        }
    }
    parts.dedup();
    Ok(parts.join(" "))
}
