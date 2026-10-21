// btleplug Source Code File
//
// Copyright 2020 Nonpolynomial. All rights reserved.
//
// Licensed under the BSD 3-Clause license. See LICENSE file in the project root
// for full license information.

use crate::api::ValueNotification;
use futures::stream::{Stream, StreamExt};
use std::pin::Pin;
use tokio::sync::broadcast::Receiver;

// Leave room for a delegate burst while the receiving task is rescheduled.
#[cfg(any(target_vendor = "apple", target_os = "windows"))]
pub(crate) const NOTIFICATION_BUFFER_CAPACITY: usize = 256;

#[allow(dead_code)]
pub fn notifications_stream_from_broadcast_receiver(
    receiver: Receiver<ValueNotification>,
) -> Pin<Box<dyn Stream<Item = ValueNotification> + Send>> {
    Box::pin(
        futures::stream::unfold(receiver, |mut receiver| async move {
            loop {
                match receiver.recv().await {
                    Ok(notification) => return Some((notification, receiver)),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        log::error!(
                            "BLE notification receiver lost data: lost_notifications={count}"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
        .fuse(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast;

    fn notification(value: u8) -> ValueNotification {
        ValueNotification {
            uuid: Default::default(),
            service_uuid: Default::default(),
            value: vec![value],
        }
    }

    #[tokio::test]
    async fn overflow_retains_the_connection_and_resumes_available_notifications() {
        let (sender, receiver) = broadcast::channel(2);
        let mut stream = notifications_stream_from_broadcast_receiver(receiver);
        sender.send(notification(0)).unwrap();
        assert_eq!(stream.next().await.unwrap().value, vec![0]);
        for value in 1..=3 {
            sender.send(notification(value)).unwrap();
        }
        assert_eq!(stream.next().await.unwrap().value, vec![2]);
        assert_eq!(stream.next().await.unwrap().value, vec![3]);
        sender.send(notification(4)).unwrap();
        assert_eq!(stream.next().await.unwrap().value, vec![4]);
        assert_eq!(sender.receiver_count(), 1);
    }

    #[tokio::test]
    async fn healthy_stream_preserves_order_and_closes_with_sender() {
        let (sender, receiver) = broadcast::channel(2);
        let mut stream = notifications_stream_from_broadcast_receiver(receiver);
        for value in 0..8 {
            sender.send(notification(value)).unwrap();
            assert_eq!(stream.next().await.unwrap().value, vec![value]);
        }
        drop(sender);
        assert!(stream.next().await.is_none());
    }
}
