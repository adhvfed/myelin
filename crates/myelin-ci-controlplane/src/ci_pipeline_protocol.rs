use crate::ci_credential_generation::CiJobCredentialWriteVersion;
use crate::ci_launch_authority::OperationalReservationWriteVersion;
use crate::job_accounting_store::CiJobAccountingWriteVersion;

pub const PRODUCTION_RESERVATION_WRITE_VERSION: OperationalReservationWriteVersion =
    OperationalReservationWriteVersion::V2;

pub const PRODUCTION_RESERVATION_QUEUE_MARKER: i16 = 2;

pub const PRODUCTION_CREDENTIAL_WRITE_VERSION: CiJobCredentialWriteVersion =
    CiJobCredentialWriteVersion::V2PhaseBound;

pub const PRODUCTION_ACCOUNTING_WRITE_VERSION: CiJobAccountingWriteVersion =
    CiJobAccountingWriteVersion::V4;
