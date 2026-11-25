//! Outbound email. Requests never talk to SMTP: they queue a sealed letter in the transaction
//! that makes the change it reports, and a worker in each Edge delivers what is queued.

mod outbox;
mod templates;
mod transport;
mod worker;

pub use outbox::MailOutbox;
pub(crate) use outbox::{outbox_migration, withdraw};
pub(crate) use templates::Letter;
pub use transport::MailSettings;
pub use worker::{DeliveryPass, DeliverySchedule, MailWorker, MailWorkerHandle};
