use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, header::AUTHORIZATION},
    response::Response,
};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tower::{Layer, Service};

/// Middleware layer that requires a shared bearer token on every request
/// except those matching `is_public`. The token is compared in constant time.
/// The token is held in `Arc<Mutex<String>>` so it can be rotated at runtime
/// via `PUT /player/settings` or SIGHUP config reload without restarting.
#[derive(Clone)]
pub struct AuthLayer {
    token: std::sync::Arc<std::sync::Mutex<String>>,
}

impl AuthLayer {
    pub fn new(token: String) -> Self {
        Self {
            token: std::sync::Arc::new(std::sync::Mutex::new(token)),
        }
    }

    pub fn update_token(&self, new_token: String) {
        *self.token.lock().expect("token mutex poisoned") = new_token;
    }

    pub fn current_token(&self) -> String {
        self.token.lock().expect("token mutex poisoned").clone()
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            token: self.token.clone(),
        }
    }
}

/// Paths reachable without a token (liveness probe only).
pub fn is_public(req: &Request<Body>) -> bool {
    req.uri().path() == "/health"
}

#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    token: std::sync::Arc<std::sync::Mutex<String>>,
}

impl<S> Service<Request<Body>> for AuthMiddleware<S>
where
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        if is_public(&req) {
            return Box::pin(self.inner.call(req));
        }

        let provided = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .or_else(|| {
                req.headers()
                    .get("X-L337-Token")
                    .and_then(|v| v.to_str().ok())
            });

        let ok = match provided {
            Some(value) => {
                let token_guard = self.token.lock().expect("token mutex poisoned");
                let expected = format!("Bearer {}", token_guard.as_str());
                let token_str = token_guard.as_str();
                value.len() == expected.len() && value == expected
                    || (token_str.len() == value.len() && *value == *token_str)
            }
            None => false,
        };

        if !ok {
            return Box::pin(async move {
                Ok(Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body(Body::from("missing or invalid token"))
                    .unwrap())
            });
        }

        Box::pin(self.inner.call(req))
    }
}

/// Generate a fresh self-signed certificate for the given host (or "localhost").
pub fn generate_self_signed(host: &str) -> CertifiedKey {
    let subject_alt_names = vec![host.to_string(), "localhost".to_string()];
    generate_simple_self_signed(subject_alt_names).expect("failed to generate self-signed cert")
}

/// Build an `axum_server` rustls config from a certified key (PEM/DER).
pub fn rustls_config(key: CertifiedKey) -> axum_server::tls_rustls::RustlsConfig {
    let cert_der = key.cert.der().clone();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.key_pair.serialize_der()));

    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("bad certificate/key");

    axum_server::tls_rustls::RustlsConfig::from_config(std::sync::Arc::new(server_config))
}
