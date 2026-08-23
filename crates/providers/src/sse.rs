//! SSE-слой: поток байтов ответа → поток payload'ов поля `data:`.
//!
//! Разбор кадров отдан `eventsource-stream` (он же корректно склеивает
//! многострочные `data:`, режет комментарии-пинги `: keep-alive` и не ломается
//! на UTF-8, разорванном между TCP-чанками). Здесь остаётся только маппинг
//! ошибок в [`ProviderError`].
//!
//! **Мид-стрим ретраев нет и не будет**: часть дельт уже отдана потребителю,
//! повтор запроса продублировал бы их. Ретраи живут до начала стрима — в
//! [`crate::openai_compat`].

use std::fmt::Display;

use eventsource_stream::{EventStreamError, Eventsource};
use futures::{Stream, StreamExt};
use mark2_core::provider::ProviderError;

/// Маркер конца потока в диалекте OpenAI (`data: [DONE]`).
pub const DONE_MARKER: &str = "[DONE]";

/// Является ли payload маркером конца потока.
pub fn is_done(data: &str) -> bool {
    data.trim() == DONE_MARKER
}

/// Поток байтов → поток `data:`-payload'ов.
///
/// Годится и для `reqwest::Response::bytes_stream()`, и для синтетического
/// потока в тестах: параметризован любым `Stream<Item = Result<impl AsRef<[u8]>, E>>`.
pub fn data_stream<S, B, E>(bytes: S) -> impl Stream<Item = Result<String, ProviderError>> + Send
where
    S: Stream<Item = Result<B, E>> + Send + 'static,
    B: AsRef<[u8]> + Send + 'static,
    E: Display + Send + 'static,
{
    bytes.eventsource().map(|event| match event {
        Ok(event) => Ok(event.data),
        Err(EventStreamError::Transport(err)) => Err(ProviderError::Network(err.to_string())),
        Err(err) => Err(ProviderError::Parse(format!("битый SSE-кадр: {err}"))),
    })
}

/// Поток `data:`-payload'ов из HTTP-ответа.
pub fn response_data_stream(
    response: reqwest::Response,
) -> impl Stream<Item = Result<String, ProviderError>> + Send {
    data_stream(response.bytes_stream())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    /// Разбить фикстуру на произвольные куски — как это делает TCP.
    fn byte_chunks(raw: &str, size: usize) -> Vec<Result<Vec<u8>, Infallible>> {
        raw.as_bytes()
            .chunks(size)
            .map(|c| Ok(c.to_vec()))
            .collect()
    }

    async fn collect(raw: &str, chunk_size: usize) -> Vec<String> {
        data_stream(futures::stream::iter(byte_chunks(raw, chunk_size)))
            .map(Result::unwrap)
            .collect()
            .await
    }

    #[tokio::test]
    async fn extracts_data_payloads_and_ignores_comments() {
        let raw = ": keep-alive\n\ndata: {\"a\":1}\n\ndata: {\"b\":2}\n\ndata: [DONE]\n\n";
        assert_eq!(
            collect(raw, 4096).await,
            vec![r#"{"a":1}"#, r#"{"b":2}"#, "[DONE]"]
        );
    }

    /// Кадры приходят разорванными по границам TCP-чанков — в том числе
    /// посреди многобайтового символа.
    #[tokio::test]
    async fn reassembles_frames_split_across_byte_chunks() {
        let raw = "data: {\"text\":\"привет\"}\n\ndata: [DONE]\n\n";
        for size in [1, 2, 3, 5, 7, 13] {
            assert_eq!(
                collect(raw, size).await,
                vec![r#"{"text":"привет"}"#, "[DONE]"],
                "разбиение по {size} байт"
            );
        }
    }

    #[tokio::test]
    async fn joins_multiline_data_fields() {
        let raw = "data: {\"a\":\ndata: 1}\n\n";
        assert_eq!(collect(raw, 4096).await, vec!["{\"a\":\n1}"]);
    }

    #[tokio::test]
    async fn named_events_and_ids_are_passed_through_as_data() {
        let raw = "event: message\nid: 42\ndata: payload\n\n";
        assert_eq!(collect(raw, 4096).await, vec!["payload"]);
    }

    #[tokio::test]
    async fn transport_error_maps_to_network_error() {
        let items: Vec<Result<Vec<u8>, std::io::Error>> = vec![
            Ok(b"data: {\"a\":1}\n\n".to_vec()),
            Err(std::io::Error::other("оборвалось соединение")),
        ];
        let out: Vec<_> = data_stream(futures::stream::iter(items)).collect().await;

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].as_ref().unwrap(), r#"{"a":1}"#);
        match out[1].as_ref().unwrap_err() {
            ProviderError::Network(msg) => assert!(msg.contains("оборвалось соединение")),
            other => panic!("ожидалась Network, получена {other:?}"),
        }
    }

    #[test]
    fn done_marker_detection_tolerates_spacing() {
        assert!(is_done("[DONE]"));
        assert!(is_done(" [DONE] "));
        assert!(!is_done(r#"{"id":"x"}"#));
        assert!(!is_done(""));
    }
}
