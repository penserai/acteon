use crate::stream::{self, StreamFilter};
use crate::{ActeonClient, Error, EventStream};

impl ActeonClient {
    /// Subscribe to the real-time SSE event stream.
    ///
    /// Returns an [`EventStream`] that yields [`StreamItem`](crate::StreamItem)s as the server
    /// emits them. Use a [`StreamFilter`] to limit which events are received.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, StreamFilter};
    /// use futures::StreamExt;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let filter = StreamFilter::new().namespace("notifications");
    ///
    /// let mut stream = client.stream(&filter).await?;
    /// while let Some(item) = stream.next().await {
    ///     match item? {
    ///         acteon_client::StreamItem::Event(event) => {
    ///             println!("Event: {:?}", event);
    ///         }
    ///         acteon_client::StreamItem::Lagged { skipped } => {
    ///             eprintln!("Missed {skipped} events");
    ///         }
    ///         acteon_client::StreamItem::KeepAlive => {}
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn stream(&self, filter: &StreamFilter) -> Result<EventStream, Error> {
        let url = format!("{}/v1/stream", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .query(filter)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(stream::event_stream_from_response(response))
        } else {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            Err(Error::Http { status, message })
        }
    }

    /// Subscribe to events for a specific entity via the
    /// `GET /v1/subscribe/{entity_type}/{entity_id}` endpoint.
    ///
    /// This is a convenience method that opens an SSE stream pre-filtered
    /// for the given entity type and ID.
    pub async fn subscribe_entity(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<EventStream, Error> {
        let url = format!(
            "{}/v1/subscribe/{}/{}",
            self.base_url, entity_type, entity_id
        );

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(stream::event_stream_from_response(response))
        } else {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            Err(Error::Http { status, message })
        }
    }

    /// Subscribe to events for a specific chain.
    pub async fn subscribe_chain(&self, chain_id: &str) -> Result<EventStream, Error> {
        self.subscribe_entity("chain", chain_id).await
    }

    /// Subscribe to events for a specific group.
    pub async fn subscribe_group(&self, group_id: &str) -> Result<EventStream, Error> {
        self.subscribe_entity("group", group_id).await
    }

    /// Subscribe to events for a specific action.
    pub async fn subscribe_action(&self, action_id: &str) -> Result<EventStream, Error> {
        self.subscribe_entity("action", action_id).await
    }
}
