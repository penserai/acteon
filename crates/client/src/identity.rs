use crate::{ActeonClient, CredentialIdentity, Error, PlatformOperation};

impl ActeonClient {
    /// Inspect the current credential and optional stable principal binding.
    pub async fn identity(&self) -> Result<CredentialIdentity, Error> {
        serde_json::from_value(
            self.platform_request(PlatformOperation::AuthIdentity, &[], &[], None)
                .await?,
        )
        .map_err(|e| Error::Deserialization(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    #[tokio::test]
    async fn typed_identity_preserves_shared_contracts() {
        let fixtures: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../clients/contract-fixtures/identity.json"
        ))
        .unwrap();
        for wire in fixtures {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let body = wire.to_string();
            let thread = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut chunk).unwrap();
                    assert_ne!(n, 0);
                    request.extend_from_slice(&chunk[..n]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("GET /v1/auth/identity HTTP/1.1"));
                assert!(request.contains("authorization: Bearer test-key"));
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
            });
            let client = ActeonClient::builder(format!("http://{addr}"))
                .api_key("test-key")
                .build()
                .unwrap();
            assert_eq!(
                serde_json::to_value(client.identity().await.unwrap()).unwrap(),
                wire
            );
            thread.join().unwrap();
        }
    }
}
