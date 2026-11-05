//! Run the diagnostic capture in its own test binary so parallel executor tests
//! cannot change tracing callsite interest while this subscriber is active.
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::Request,
    response::Response,
    routing::any,
};
use serde_json::{Value, json};
use std::{
    io,
    sync::{Arc, Mutex},
};
use tines_mcp::{
    config::Config,
    tines::TinesClient,
    tools::{self, contract::LocalLimits},
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

type Requests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);
struct LogWriter(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}

#[tokio::test]
async fn diagnostics_exclude_credentials_payloads_ids_queries_and_native_errors() {
    let requests = Arc::new(Mutex::new(Vec::<(String, Vec<u8>)>::new()));
    async fn handler(State(requests): State<Requests>, request: Request<Body>) -> Response {
        assert_eq!(request.headers()["authorization"], "Bearer executor-secret");
        let uri = request.uri().to_string();
        let bytes = to_bytes(request.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec();
        requests.lock().unwrap().push((uri, bytes));
        Response::builder().status(403).body(Body::from(r#"{"error":{"code":"denied","message":"private-native-message executor-secret","details":{"body":"private-native-body"}}}"#)).unwrap()
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(any(handler))
        .with_state(requests.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let config = Config::from_lookup(|key| match key {
        "TINES_BASE_URL" => Some(url.clone()),
        "TINES_API_TOKEN" => Some("executor-secret".into()),
        "TINES_ALLOW_HTTP" => Some("true".into()),
        _ => None,
    })
    .unwrap();
    let client = TinesClient::new(config.tines.as_ref().unwrap()).unwrap();
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(logs.clone())
        .without_time()
        .with_ansi(false)
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    for (name, args) in [
        (
            "append_issue_comment",
            json!({"issue_id":"private-path-id","body":"private-request-body"}),
        ),
        ("list_issues", json!({"search":"private-query-value"})),
    ] {
        let result = tools::dispatch(
            &client,
            name,
            args.as_object().unwrap().clone(),
            &LocalLimits::default(),
            CancellationToken::new(),
        )
        .await;
        let output = serde_json::to_string(&result).unwrap();
        assert!(!output.contains("executor-secret"));
        assert!(output.contains("[REDACTED]"));
    }
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].0.contains("private-path-id"));
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].1).unwrap()["body"],
        "private-request-body"
    );
    assert!(requests[1].0.contains("private-query-value"));
    let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for private in [
        "executor-secret",
        "private-request-body",
        "private-native-body",
        "private-native-message",
        "private-path-id",
        "private-query-value",
    ] {
        assert!(!logs.contains(private), "diagnostics leaked {private}");
    }
    assert!(logs.contains("/issues/{issue_id}/comments"), "{logs:?}");
    assert!(logs.contains("request_count=1"));
    assert!(logs.contains("Unknown"));
    task.abort();
}
