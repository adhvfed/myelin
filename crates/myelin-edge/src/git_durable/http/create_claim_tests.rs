use super::*;
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_tenancy::{Region as IdRegion, TenantId};
use std::sync::{mpsc, Barrier, Mutex};
use std::time::Duration;

fn principal(id: &str, tenant: &str) -> Principal {
    Principal::new(
        TenantId(tenant.into()),
        IdRegion("fr-par".into()),
        PrincipalId(id.into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
    )
}

fn temp_root(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "myelin-create-claim-{tag}-{}-{nanos}",
        std::process::id()
    ))
}

#[derive(Default)]
struct RecordingBootstrap {
    grants: Mutex<Vec<(String, String)>>,
}

impl RepoBootstrapGrants for RecordingBootstrap {
    fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
        self.grants
            .lock()
            .unwrap()
            .push((creator.principal_id.0.clone(), repo.repo.clone()));
        Ok(())
    }
}

struct CommitThenDisconnectBootstrap {
    grants: Mutex<Vec<(String, String)>>,
}

impl RepoBootstrapGrants for CommitThenDisconnectBootstrap {
    fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
        self.grants
            .lock()
            .unwrap()
            .push((creator.principal_id.0.clone(), repo.repo.clone()));
        Err("the durable grant committed, but its response was lost".into())
    }
}

struct PausingBootstrap {
    grants: Mutex<Vec<(String, String)>>,
    first_grant_entered: mpsc::Sender<()>,
    release_first_grant: Mutex<mpsc::Receiver<()>>,
}

impl RepoBootstrapGrants for PausingBootstrap {
    fn grant_creator(&self, creator: &Principal, repo: &RepoLoc) -> Result<(), String> {
        let is_first = {
            let mut grants = self.grants.lock().unwrap();
            grants.push((creator.principal_id.0.clone(), repo.repo.clone()));
            grants.len() == 1
        };
        if is_first {
            self.first_grant_entered.send(()).unwrap();
            self.release_first_grant
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))
                .expect("the test releases the first durable grant");
        }
        Ok(())
    }
}

#[test]
fn successful_create_grants_its_creator_once() {
    let root = temp_root("ok");
    let boot = Arc::new(RecordingBootstrap::default());
    let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(boot.clone());
    let creator = principal("svc:creator", "acme");

    let created = be
        .create_repo_as("acme", "fr-par", "widgets", &creator)
        .expect("create succeeds");
    assert!(created);
    assert_eq!(boot.grants.lock().unwrap().len(), 1, "granted once");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_ambiguous_grant_response_keeps_the_slug_with_its_original_creator() {
    let root = temp_root("resume");
    let creator = principal("svc:creator", "acme");
    let stranger = principal("svc:stranger", "acme");
    let interrupted_grant = Arc::new(CommitThenDisconnectBootstrap {
        grants: Mutex::new(Vec::new()),
    });
    let interrupted = DurableGitBackend::rooted_inmem_for_test(&root)
        .with_repo_bootstrap(interrupted_grant.clone());

    let interrupted_error = interrupted
        .create_repo_as("acme", "fr-par", "widgets", &creator)
        .expect_err("the caller lost the committed grant response");
    assert!(interrupted_error
        .to_string()
        .contains("owner-bound repository claim remains retryable"));
    assert_eq!(
        *interrupted_grant.grants.lock().unwrap(),
        vec![("svc:creator".to_string(), "widgets".to_string())]
    );

    let stranger_grants = Arc::new(RecordingBootstrap::default());
    let after_restart = DurableGitBackend::rooted_inmem_for_test(&root)
        .with_repo_bootstrap(stranger_grants.clone());
    let conflict = after_restart
        .create_repo_as("acme", "fr-par", "widgets", &stranger)
        .expect_err("another principal cannot adopt the interrupted slug");
    assert!(matches!(conflict, DurableError::Conflict(_)));
    assert_eq!(
        stranger_grants.grants.lock().unwrap().len(),
        0,
        "the stranger never reaches authorization"
    );

    let resumed_grants = Arc::new(RecordingBootstrap::default());
    let resumed =
        DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(resumed_grants.clone());
    assert!(resumed
        .create_repo_as("acme", "fr-par", "widgets", &creator)
        .expect("the original creator resumes after restart"));
    assert_eq!(
        *resumed_grants.grants.lock().unwrap(),
        vec![("svc:creator".to_string(), "widgets".to_string())]
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn simultaneous_creators_cannot_both_claim_the_same_repository() {
    let root = temp_root("concurrent");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let bootstrap = Arc::new(PausingBootstrap {
        grants: Mutex::new(Vec::new()),
        first_grant_entered: entered_tx,
        release_first_grant: Mutex::new(release_rx),
    });
    let backend = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(bootstrap.clone()),
    );
    let starting_line = Arc::new(Barrier::new(3));

    let spawn_creator = |id: &'static str| {
        let backend = backend.clone();
        let starting_line = starting_line.clone();
        std::thread::spawn(move || {
            let creator = principal(id, "acme");
            starting_line.wait();
            backend.create_repo_as("acme", "fr-par", "widgets", &creator)
        })
    };
    let first = spawn_creator("svc:alice");
    let second = spawn_creator("svc:bob");
    starting_line.wait();
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("one creator enters the grant while holding the claim");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        bootstrap.grants.lock().unwrap().len(),
        1,
        "the other creator waits outside the authorization boundary"
    );
    release_tx.send(()).unwrap();

    let mut outcomes = vec![
        first.join().unwrap().unwrap(),
        second.join().unwrap().unwrap(),
    ];
    outcomes.sort_unstable();
    assert_eq!(outcomes, vec![false, true]);
    assert_eq!(
        bootstrap.grants.lock().unwrap().len(),
        1,
        "exactly the creator that initialized the repository received admin"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_existing_repo_does_not_grant_again() {
    let root = temp_root("exists");
    let boot = Arc::new(RecordingBootstrap::default());
    let be = DurableGitBackend::rooted_inmem_for_test(&root).with_repo_bootstrap(boot.clone());
    let creator = principal("svc:creator", "acme");
    assert!(be
        .create_repo_as("acme", "fr-par", "widgets", &creator)
        .unwrap());
    assert!(!be
        .create_repo_as("acme", "fr-par", "widgets", &creator)
        .unwrap());
    assert_eq!(
        boot.grants.lock().unwrap().len(),
        1,
        "granted only on the first create"
    );
    std::fs::remove_dir_all(&root).ok();
}
