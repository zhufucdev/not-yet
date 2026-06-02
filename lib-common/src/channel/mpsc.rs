use tokio::sync::mpsc::{Sender, channel};

pub trait MapSender<I, T, F> {
    fn map(self, f: F) -> Sender<I>;
}

impl<I, T, F> MapSender<I, T, F> for Sender<T>
where
    T: Send + 'static,
    F: Fn(I) -> T + Send + 'static,
    I: Send + 'static,
{
    fn map(self, f: F) -> Sender<I> {
        let (tx, mut rx) = channel(1);
        tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                self.send(f(data)).await;
            }
        });
        tx
    }
}

#[cfg(test)]
mod test {
    use super::MapSender;
    use tokio::sync::mpsc::channel;

    #[tokio::test]
    async fn maps_sent_values() {
        let (target_tx, mut target_rx) = channel(1);
        let mapped_tx = target_tx.map(|value: &str| value.len());

        mapped_tx.send("hello").await.unwrap();

        assert_eq!(target_rx.recv().await, Some(5));
    }

    #[tokio::test]
    async fn preserves_send_order() {
        let (target_tx, mut target_rx) = channel(2);
        let mapped_tx = target_tx.map(|value| value * 2);

        mapped_tx.send(1).await.unwrap();
        mapped_tx.send(2).await.unwrap();

        assert_eq!(target_rx.recv().await, Some(2));
        assert_eq!(target_rx.recv().await, Some(4));
    }

    #[tokio::test]
    async fn closes_target_when_mapped_sender_is_dropped() {
        let (target_tx, mut target_rx) = channel(1);
        let mapped_tx = target_tx.map(|value| value + 1);

        mapped_tx.send(1).await.unwrap();
        drop(mapped_tx);

        assert_eq!(target_rx.recv().await, Some(2));
        assert_eq!(target_rx.recv().await, None);
    }
}
