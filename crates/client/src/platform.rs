//! Access every registered finite HTTP operation using server wire contracts.
use crate::{ActeonClient, Error, PlatformOperation};
use serde_json::Value;

impl ActeonClient {
    /// Call a registered operation with opaque path segments, query values and
    /// a wire-format JSON body. JSON envelopes are preserved, text is returned
    /// as a JSON string, and HTTP 204 as null. No automatic retry is performed.
    /// Retain stable request IDs when retrying session/stage operations.
    pub async fn platform_request(
        &self,
        operation: PlatformOperation,
        path_parameters: &[(&str, &str)],
        query: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Value, Error> {
        let (method, template, parameters, text) = operation.descriptor();
        if path_parameters.len() != parameters.len() {
            return Err(Error::Configuration(format!(
                "expected path parameters: {parameters:?}"
            )));
        }
        let mut path = template.to_owned();
        for name in parameters {
            let values: Vec<_> = path_parameters
                .iter()
                .filter(|(key, _)| key == name)
                .collect();
            if values.len() != 1 || matches!(values[0].1, "" | "." | "..") {
                return Err(Error::Configuration(format!(
                    "invalid path parameter: {name}"
                )));
            }
            let encoded = percent_encoding::utf8_percent_encode(
                values[0].1,
                percent_encoding::NON_ALPHANUMERIC,
            )
            .to_string();
            path = path.replace(&format!("{{{name}}}"), &encoded);
        }
        if method == "GET" && body.is_some() {
            return Err(Error::Configuration(
                "GET operations do not accept a body".into(),
            ));
        }
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| Error::Configuration(e.to_string()))?;
        let mut request = self
            .add_auth(
                self.client
                    .request(method, format!("{}{path}", self.base_url)),
            )
            .query(query);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        let status = response.status();
        let content = response
            .text()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if !status.is_success() {
            return Err(Error::Http {
                status: status.as_u16(),
                message: content,
            });
        }
        if status.as_u16() == 204 {
            return Ok(Value::Null);
        }
        if text {
            return Ok(Value::String(content));
        }
        serde_json::from_str(&content).map_err(|e| Error::Deserialization(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[tokio::test]
    async fn control_preserves_auth_method_body_and_status() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 4096];
            loop {
                let n = stream.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if body.len() == length {
                        break;
                    }
                }
            }
            stream.write_all(b"HTTP/1.1 409 Conflict\r\nContent-Length: 8\r\nConnection: close\r\n\r\nconflict").unwrap();
            String::from_utf8(request).unwrap()
        });
        let client = ActeonClient::builder(format!("http://{address}"))
            .api_key("test-key")
            .build()
            .unwrap();
        let error = client
            .platform_request(
                PlatformOperation::BusStagesControl,
                &[("namespace", "n"), ("tenant", "t"), ("id", "a/b ?#%")],
                &[("filter", "a b"), ("filter", "c&d")],
                Some(&serde_json::json!({"request_id":"stable"})),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Http { status: 409, .. }));
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /v1/bus/stages/n/t/a%2Fb%20%3F%23%25/control?filter=a+b&filter=c%26d HTTP/1.1\r\n"), "{request}");
        assert!(request.contains("authorization: Bearer test-key\r\n"));
        assert!(request.ends_with("{\"request_id\":\"stable\"}"));
        assert!(
            client
                .platform_request(
                    PlatformOperation::BusStagesStatus,
                    &[("namespace", "n"), ("tenant", "t"), ("id", "..")],
                    &[],
                    None
                )
                .await
                .is_err()
        );
    }

    #[test]
    fn governance_fixtures_match_server_wire_format() {
        let fixtures: Value = serde_json::from_str(include_str!(
            "../../../clients/contract-fixtures/dispatch-outcomes.json"
        ))
        .unwrap();
        for fixture in fixtures.as_array().unwrap() {
            let outcome: acteon_core::ActionOutcome =
                serde_json::from_value(fixture["wire"].clone()).unwrap();
            assert_eq!(serde_json::to_value(outcome).unwrap(), fixture["wire"]);
        }
    }
}
