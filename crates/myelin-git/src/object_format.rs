use myelin_storage::{git_object_address, ContentHash, GitObjectKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectFormat {
    Sha1Dc,
    Sha256,
}

impl ObjectFormat {
    pub fn config_token(self) -> &'static str {
        match self {
            ObjectFormat::Sha1Dc => "sha1",
            ObjectFormat::Sha256 => "sha256",
        }
    }

    pub fn oid_hex_width(self) -> usize {
        match self {
            ObjectFormat::Sha1Dc => 40,
            ObjectFormat::Sha256 => 64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewRepoDefault {
    format: ObjectFormat,
}

impl NewRepoDefault {
    pub const fn sha1dc_floor() -> NewRepoDefault {
        NewRepoDefault {
            format: ObjectFormat::Sha1Dc,
        }
    }

    const fn sha256_flipped() -> NewRepoDefault {
        NewRepoDefault {
            format: ObjectFormat::Sha256,
        }
    }

    pub fn format(self) -> ObjectFormat {
        self.format
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InteropBar {
    pub stock_git_reads_sha256: bool,
    pub ci_runners_read_sha256: bool,
    pub ide_tooling_reads_sha256: bool,
}

impl InteropBar {
    pub fn is_met(self) -> bool {
        self.stock_git_reads_sha256 && self.ci_runners_read_sha256 && self.ide_tooling_reads_sha256
    }
}

pub fn flip_default_to_sha256(bar: InteropBar) -> Result<NewRepoDefault, FlipRefused> {
    if bar.is_met() {
        Ok(NewRepoDefault::sha256_flipped())
    } else {
        Err(FlipRefused { bar })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlipRefused {
    pub bar: InteropBar,
}

impl std::fmt::Display for FlipRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the SHA-256 default flip (OQ-9) is REFUSED: the stock-tooling interop bar is not met \
             (stock_git={}, ci_runners={}, ide_tooling={}). The new-repo default stays SHA-1+sha1dc \
             - a SHA-256-default repo would fail to interoperate until every tooling lane reads it.",
            self.bar.stock_git_reads_sha256,
            self.bar.ci_runners_read_sha256,
            self.bar.ide_tooling_reads_sha256
        )
    }
}

impl std::error::Error for FlipRefused {}

pub fn create_repo_format(
    default: NewRepoDefault,
    requested: Option<ObjectFormat>,
) -> ObjectFormat {
    requested.unwrap_or_else(|| default.format())
}

pub fn object_address_for_format(
    _format: ObjectFormat,
    kind: GitObjectKind,
    content: &[u8],
) -> ContentHash {
    git_object_address(kind, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar_all_ready() -> InteropBar {
        InteropBar {
            stock_git_reads_sha256: true,
            ci_runners_read_sha256: true,
            ide_tooling_reads_sha256: true,
        }
    }

    #[test]
    fn new_repo_defaults_to_the_system_default_when_unpinned() {
        let floor = NewRepoDefault::sha1dc_floor();
        assert_eq!(create_repo_format(floor, None), ObjectFormat::Sha1Dc);

        let flipped = flip_default_to_sha256(bar_all_ready()).expect("bar met → flip");
        assert_eq!(create_repo_format(flipped, None), ObjectFormat::Sha256);
    }

    #[test]
    fn after_the_flip_a_new_repo_defaults_sha256_existing_repos_unchanged() {
        let existing_format = create_repo_format(NewRepoDefault::sha1dc_floor(), None);
        assert_eq!(existing_format, ObjectFormat::Sha1Dc);

        let flipped = flip_default_to_sha256(bar_all_ready()).expect("bar met");

        assert_eq!(create_repo_format(flipped, None), ObjectFormat::Sha256);

        assert_eq!(
            existing_format,
            ObjectFormat::Sha1Dc,
            "an existing repo's object format is untouched by the default flip (not a migration)"
        );
    }

    #[test]
    fn explicit_per_repo_pin_wins_over_the_default() {
        let floor = NewRepoDefault::sha1dc_floor();
        assert_eq!(
            create_repo_format(floor, Some(ObjectFormat::Sha256)),
            ObjectFormat::Sha256,
            "an explicit SHA-256 pin opts a repo in before the global flip"
        );

        let flipped = flip_default_to_sha256(bar_all_ready()).unwrap();
        assert_eq!(
            create_repo_format(flipped, Some(ObjectFormat::Sha1Dc)),
            ObjectFormat::Sha1Dc,
            "an explicit SHA-1 pin opts a repo out after the flip"
        );
    }

    #[test]
    fn flip_is_refused_when_the_interop_bar_is_unmet() {
        for unready in [
            InteropBar {
                stock_git_reads_sha256: false,
                ..bar_all_ready()
            },
            InteropBar {
                ci_runners_read_sha256: false,
                ..bar_all_ready()
            },
            InteropBar {
                ide_tooling_reads_sha256: false,
                ..bar_all_ready()
            },
        ] {
            let refused =
                flip_default_to_sha256(unready).expect_err("an unmet bar refuses the flip");
            assert_eq!(refused.bar, unready);
            assert!(refused.to_string().contains("REFUSED"));
        }
    }

    #[test]
    fn interop_bar_is_all_lanes_and() {
        assert!(bar_all_ready().is_met(), "all lanes ready → bar met");
        assert!(!InteropBar {
            stock_git_reads_sha256: false,
            ..bar_all_ready()
        }
        .is_met());
        assert!(!InteropBar {
            ci_runners_read_sha256: false,
            ..bar_all_ready()
        }
        .is_met());
        assert!(!InteropBar {
            ide_tooling_reads_sha256: false,
            ..bar_all_ready()
        }
        .is_met());
        assert!(!InteropBar {
            stock_git_reads_sha256: false,
            ci_runners_read_sha256: false,
            ide_tooling_reads_sha256: false,
        }
        .is_met());
    }

    #[test]
    fn format_carries_its_config_token_and_width() {
        assert_eq!(ObjectFormat::Sha1Dc.config_token(), "sha1");
        assert_eq!(ObjectFormat::Sha256.config_token(), "sha256");
        assert_eq!(ObjectFormat::Sha1Dc.oid_hex_width(), 40);
        assert_eq!(ObjectFormat::Sha256.oid_hex_width(), 64);
        assert_ne!(
            ObjectFormat::Sha1Dc.oid_hex_width(),
            ObjectFormat::Sha256.oid_hex_width()
        );
    }

    #[test]
    fn object_addressing_threads_the_format() {
        let content = b"fn main() {}\n";
        let a = object_address_for_format(ObjectFormat::Sha256, GitObjectKind::Blob, content);
        let b = object_address_for_format(ObjectFormat::Sha1Dc, GitObjectKind::Blob, content);
        assert_eq!(a, git_object_address(GitObjectKind::Blob, content));
        assert_eq!(
            a, b,
            "the storage framing is SHA-256 on this floor (format carried, not assumed)"
        );
    }

    #[test]
    fn format_is_immutable_no_set_format_exists() {
        let f = create_repo_format(NewRepoDefault::sha1dc_floor(), Some(ObjectFormat::Sha256));
        assert_eq!(
            f,
            create_repo_format(NewRepoDefault::sha1dc_floor(), Some(ObjectFormat::Sha256))
        );
    }
}
