//! Delivers queued mail. Delivery is at-least-once: a worker that sends and then dies before it
//! deletes the row leaves it to be sent again once its claim runs out. Every retry keeps the
//! row's Message-ID, so a receiving system can tell the copies are one message.

use std::sync::Arc;
use std::time::Duration;

use lettre::message::{Mailbox, MultiPart};
use lettre::Message;
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::outbox::{AfterFailure, Claimed, MailOutbox, MessageId};
use super::templates::Letter;
use super::transport::{MailSettings, Mailer, SendFailure};

#[derive(Clone, Debug)]
pub struct DeliverySchedule {
    /// How long one worker holds a letter while it tries to send it. Longer than `send_deadline`,
    /// so a send that runs out of time ends before another worker may take its letter.
    pub claim_secs: i64,
    /// The most one send may take, from connecting to the relay's last reply.
    pub send_deadline: Duration,
    /// The first retry waits this long; each later one twice as long, up to `max_retry_secs`.
    pub first_retry_secs: i64,
    pub max_retry_secs: i64,
    /// A letter still failing this long after it was queued is dropped.
    pub give_up_after_secs: i64,
    /// How often an idle worker looks for due mail.
    pub poll: Duration,
}

impl Default for DeliverySchedule {
    fn default() -> Self {
        Self {
            claim_secs: 5 * 60,
            send_deadline: Duration::from_secs(60),
            first_retry_secs: 30,
            max_retry_secs: 60 * 60,
            give_up_after_secs: 24 * 60 * 60,
            poll: Duration::from_secs(2),
        }
    }
}

impl DeliverySchedule {
    fn retry_in_secs(&self, attempts: i32) -> i64 {
        let doublings = u32::try_from(attempts.saturating_sub(1))
            .unwrap_or(0)
            .min(20);
        self.first_retry_secs
            .saturating_mul(1_i64 << doublings)
            .min(self.max_retry_secs)
    }
}

/// What one pass over the due mail did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeliveryPass {
    pub sent: usize,
    pub retrying: usize,
    pub dropped: usize,
}

pub struct MailWorker {
    pool: PgPool,
    outbox: MailOutbox,
    mailer: Mailer,
    schedule: DeliverySchedule,
}

impl MailWorker {
    pub fn new(
        pool: PgPool,
        outbox: MailOutbox,
        settings: &MailSettings,
        schedule: DeliverySchedule,
    ) -> Result<Self, String> {
        Ok(Self {
            pool,
            outbox,
            mailer: Mailer::new(settings)?,
            schedule,
        })
    }

    /// Drops letters queued longer than the schedule allows, then tries the letters that are due
    /// now, up to a bounded number per pass.
    pub async fn deliver_due(&self) -> Result<DeliveryPass, sqlx::Error> {
        const MAX_PER_PASS: usize = 64;
        let mut pass = DeliveryPass::default();
        for (id, attempts) in self
            .outbox
            .drop_expired(&self.pool, self.schedule.give_up_after_secs)
            .await?
        {
            log_dropped(&id, attempts, "queued too long");
            pass.dropped += 1;
        }
        for _ in 0..MAX_PER_PASS {
            let Some(claimed) = self
                .outbox
                .claim(&self.pool, self.schedule.claim_secs)
                .await?
            else {
                break;
            };
            // A letter this edge cannot open may be one a newer edge wrote, so it waits for a
            // reader until its time is up rather than being dropped.
            let Some(letter) = &claimed.letter else {
                let unreadable = SendFailure {
                    kind: "unreadable",
                    code: None,
                };
                match self.failed(&claimed, &unreadable).await? {
                    AfterFailure::GaveUp => pass.dropped += 1,
                    AfterFailure::Retrying | AfterFailure::NotOurs => pass.retrying += 1,
                }
                continue;
            };
            if !claimed.link_live {
                self.outbox.failed(&self.pool, &claimed, 0, 0).await?;
                log_dropped(
                    &claimed.id,
                    claimed.attempts,
                    "its link has expired or was used",
                );
                pass.dropped += 1;
                continue;
            }
            match message(self.mailer.from(), &claimed.id, letter) {
                Ok(message) => match self.mailer.send(message, self.schedule.send_deadline).await {
                    Ok(()) => {
                        self.outbox.sent(&self.pool, &claimed.id).await?;
                        pass.sent += 1;
                    }
                    Err(failure) => match self.failed(&claimed, &failure).await? {
                        AfterFailure::GaveUp => pass.dropped += 1,
                        AfterFailure::Retrying | AfterFailure::NotOurs => pass.retrying += 1,
                    },
                },
                Err(Undeliverable(reason)) => {
                    self.outbox.failed(&self.pool, &claimed, 0, 0).await?;
                    log_dropped(&claimed.id, claimed.attempts, reason);
                    pass.dropped += 1;
                }
            }
        }
        Ok(pass)
    }

    async fn failed(
        &self,
        claimed: &Claimed,
        failure: &SendFailure,
    ) -> Result<AfterFailure, sqlx::Error> {
        let retry_in_secs = self.schedule.retry_in_secs(claimed.attempts);
        let after = self
            .outbox
            .failed(
                &self.pool,
                claimed,
                retry_in_secs,
                self.schedule.give_up_after_secs,
            )
            .await?;
        let event = if after == AfterFailure::GaveUp {
            "edge.mail.undeliverable"
        } else {
            "edge.mail.deferred"
        };
        eprintln!(
            "{}",
            json!({
                "event": event,
                "message_id": hex(&claimed.id),
                "attempt": claimed.attempts,
                "failure": failure.kind,
                "smtp_code": failure.code,
                "retry_in_secs": (after == AfterFailure::Retrying).then_some(retry_in_secs),
            })
        );
        Ok(after)
    }

    /// Runs passes until shutdown. Each pass is its own task, so a panic costs one pass, not the
    /// worker; shutdown abandons a pass mid-send, and its letter is retried once its claim ends.
    pub fn spawn(self) -> MailWorkerHandle {
        let (shutdown, mut stopping) = watch::channel(false);
        let worker = Arc::new(self);
        let join = tokio::spawn(async move {
            loop {
                let pass = tokio::spawn({
                    let worker = worker.clone();
                    async move { worker.deliver_due().await }
                });
                let abort = pass.abort_handle();
                let failure = tokio::select! {
                    _ = stopping.changed() => {
                        abort.abort();
                        return;
                    }
                    finished = pass => match finished {
                        Ok(Ok(_)) => None,
                        Ok(Err(error)) => Some(error.to_string()),
                        Err(error) => Some(format!("a delivery pass stopped: {error}")),
                    },
                };
                if let Some(error) = failure {
                    eprintln!(
                        "{}",
                        json!({ "event": "edge.mail.outbox_unavailable", "error": error })
                    );
                }
                tokio::select! {
                    _ = stopping.changed() => return,
                    _ = tokio::time::sleep(worker.schedule.poll) => {}
                }
            }
        });
        MailWorkerHandle { shutdown, join }
    }
}

/// Stops the worker when shut down or dropped: dropping it closes the shutdown channel, which the
/// worker reads as a request to stop.
#[must_use = "dropping the handle stops the mail worker"]
pub struct MailWorkerHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl MailWorkerHandle {
    /// Stops at once, abandoning any send in progress. That letter is still queued, so it is sent
    /// again once its claim runs out: nothing is lost, though it may arrive twice.
    pub async fn shutdown(self) -> Result<(), String> {
        let _ = self.shutdown.send(true);
        self.join
            .await
            .map_err(|error| format!("the mail worker failed: {error}"))
    }
}

struct Undeliverable(&'static str);

fn message(from: &Mailbox, id: &MessageId, letter: &Letter) -> Result<Message, Undeliverable> {
    let to: Mailbox = letter
        .to()
        .parse()
        .map_err(|_| Undeliverable("the recipient address cannot be used in SMTP"))?;
    let rendered = letter.render();
    Message::builder()
        .from(from.clone())
        .to(to)
        .subject(rendered.subject)
        .message_id(Some(format!("<{}@{}>", hex(id), from.email.domain())))
        .multipart(MultiPart::alternative_plain_html(
            rendered.text,
            rendered.html,
        ))
        .map_err(|_| Undeliverable("the message cannot be built"))
}

fn log_dropped(id: &MessageId, attempts: i32, reason: &str) {
    eprintln!(
        "{}",
        json!({
            "event": "edge.mail.undeliverable",
            "message_id": hex(id),
            "attempt": attempts,
            "failure": reason,
        })
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off_to_an_hour() {
        let schedule = DeliverySchedule::default();
        let waits: Vec<i64> = (1..=10).map(|n| schedule.retry_in_secs(n)).collect();
        assert_eq!(waits, [30, 60, 120, 240, 480, 960, 1920, 3600, 3600, 3600]);
        assert_eq!(schedule.retry_in_secs(i32::MAX), 3600);
    }

    #[test]
    fn a_message_keeps_its_id_and_writes_the_link_out() {
        let from: Mailbox = "Myelin <no-reply@myelin.example>".parse().unwrap();
        let letter = Letter::PasswordReset {
            to: "ada@example.com".into(),
            link: "https://myelin.example/join/abc".into(),
            expires_at_unix: 1_796_220_300,
        };
        let id = [0xab; 16];
        let first =
            String::from_utf8(message(&from, &id, &letter).ok().unwrap().formatted()).unwrap();
        assert!(first.contains(&format!("Message-ID: <{}@myelin.example>", "ab".repeat(16))));
        assert!(first.contains("To: ada@example.com"));
        assert!(first.contains("https://myelin.example/join/abc"));
        assert!(message(
            &from,
            &id,
            &Letter::PasswordChanged {
                to: "not an address".into(),
                sign_in: None
            }
        )
        .is_err());
    }
}
