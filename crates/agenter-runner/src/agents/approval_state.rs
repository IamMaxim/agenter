use agenter_core::ApprovalDecision;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};

#[derive(Debug)]
pub struct ProviderApprovalDecision {
    pub decision: ApprovalDecision,
    pub acknowledged: oneshot::Sender<Result<(), String>>,
}

#[derive(Clone, Debug)]
pub struct PendingProviderApproval {
    inner: Arc<Mutex<PendingProviderApprovalState>>,
}

#[derive(Debug)]
struct PendingProviderApprovalState {
    provider_response: Option<oneshot::Sender<ProviderApprovalDecision>>,
    in_flight: Option<InFlightApprovalDecision>,
    completed: Option<CompletedApprovalDecision>,
}

#[derive(Debug)]
struct InFlightApprovalDecision {
    decision: ApprovalDecision,
    waiters: Vec<oneshot::Sender<Result<(), String>>>,
}

#[derive(Clone, Debug)]
struct CompletedApprovalDecision {
    decision: ApprovalDecision,
    result: Result<(), String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PendingApprovalSubmitError {
    ConflictingDecision,
    ProviderWaiterDropped,
    ProviderRejected(String),
    AcknowledgementDropped,
}

impl PendingProviderApproval {
    #[must_use]
    pub fn new(provider_response: oneshot::Sender<ProviderApprovalDecision>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(PendingProviderApprovalState {
                provider_response: Some(provider_response),
                in_flight: None,
                completed: None,
            })),
        }
    }

    pub async fn submit(
        &self,
        decision: ApprovalDecision,
    ) -> Result<(), PendingApprovalSubmitError> {
        let waiter = {
            let mut state = self.inner.lock().await;

            if let Some(completed) = &state.completed {
                if completed.decision == decision {
                    return completed
                        .result
                        .clone()
                        .map_err(PendingApprovalSubmitError::ProviderRejected);
                }
                return Err(PendingApprovalSubmitError::ConflictingDecision);
            }

            if let Some(in_flight) = &mut state.in_flight {
                if in_flight.decision != decision {
                    return Err(PendingApprovalSubmitError::ConflictingDecision);
                }
                let (waiter_sender, waiter_receiver) = oneshot::channel();
                in_flight.waiters.push(waiter_sender);
                waiter_receiver
            } else {
                let Some(provider_response) = state.provider_response.take() else {
                    return Err(PendingApprovalSubmitError::ProviderWaiterDropped);
                };
                let (waiter_sender, waiter_receiver) = oneshot::channel();
                let (ack_sender, ack_receiver) = oneshot::channel();
                state.in_flight = Some(InFlightApprovalDecision {
                    decision: decision.clone(),
                    waiters: vec![waiter_sender],
                });
                if provider_response
                    .send(ProviderApprovalDecision {
                        decision,
                        acknowledged: ack_sender,
                    })
                    .is_err()
                {
                    state.in_flight = None;
                    return Err(PendingApprovalSubmitError::ProviderWaiterDropped);
                }
                let pending = self.clone();
                tokio::spawn(async move {
                    let result = ack_receiver
                        .await
                        .unwrap_or(Err("provider acknowledgement channel dropped".to_owned()));
                    pending.complete(result).await;
                });
                waiter_receiver
            }
        };

        waiter
            .await
            .map_err(|_| PendingApprovalSubmitError::AcknowledgementDropped)?
            .map_err(PendingApprovalSubmitError::ProviderRejected)
    }

    async fn complete(&self, result: Result<(), String>) {
        let mut state = self.inner.lock().await;
        let Some(in_flight) = state.in_flight.take() else {
            return;
        };
        state.completed = Some(CompletedApprovalDecision {
            decision: in_flight.decision,
            result: result.clone(),
        });
        for waiter in in_flight.waiters {
            waiter.send(result.clone()).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn duplicate_same_decision_joins_in_flight_delivery() {
        let (provider_sender, provider_receiver) = oneshot::channel();
        let pending = PendingProviderApproval::new(provider_sender);

        let first = tokio::spawn({
            let pending = pending.clone();
            async move { pending.submit(ApprovalDecision::Accept).await }
        });

        let provider_decision = provider_receiver
            .await
            .expect("provider should receive first decision");
        assert_eq!(provider_decision.decision, ApprovalDecision::Accept);

        let second = tokio::spawn({
            let pending = pending.clone();
            async move { pending.submit(ApprovalDecision::Accept).await }
        });

        provider_decision
            .acknowledged
            .send(Ok(()))
            .expect("acknowledge provider delivery");

        assert_eq!(first.await.expect("join first"), Ok(()));
        assert_eq!(second.await.expect("join second"), Ok(()));
    }

    #[tokio::test]
    async fn conflicting_duplicate_decision_is_rejected_without_second_delivery() {
        let (provider_sender, provider_receiver) = oneshot::channel();
        let pending = PendingProviderApproval::new(provider_sender);

        let first = tokio::spawn({
            let pending = pending.clone();
            async move { pending.submit(ApprovalDecision::Accept).await }
        });

        let provider_decision = provider_receiver
            .await
            .expect("provider should receive first decision");

        let conflict = pending.submit(ApprovalDecision::Decline).await;
        assert!(matches!(
            conflict,
            Err(PendingApprovalSubmitError::ConflictingDecision)
        ));

        provider_decision
            .acknowledged
            .send(Ok(()))
            .expect("acknowledge provider delivery");
        assert_eq!(first.await.expect("join first"), Ok(()));
    }

    #[tokio::test]
    async fn completed_decision_is_replayed_for_late_duplicate() {
        let (provider_sender, provider_receiver) = oneshot::channel();
        let pending = PendingProviderApproval::new(provider_sender);

        let first = tokio::spawn({
            let pending = pending.clone();
            async move { pending.submit(ApprovalDecision::Accept).await }
        });

        let provider_decision = provider_receiver
            .await
            .expect("provider should receive first decision");
        provider_decision
            .acknowledged
            .send(Ok(()))
            .expect("acknowledge provider delivery");
        assert_eq!(first.await.expect("join first"), Ok(()));

        assert_eq!(pending.submit(ApprovalDecision::Accept).await, Ok(()));
        assert!(matches!(
            pending.submit(ApprovalDecision::Cancel).await,
            Err(PendingApprovalSubmitError::ConflictingDecision)
        ));
    }
}
