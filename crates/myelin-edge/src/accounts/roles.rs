//! What each role in an organization may do to the people in it. The rules are pure; the store
//! applies them under the organization's lock, against roles read in the same transaction.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OrgRole {
    Owner,
    Admin,
    Member,
}

impl OrgRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "member" => Some(Self::Member),
            _ => None,
        }
    }

    /// Owners and admins invite people, list pending invitations and revoke them.
    pub fn manages_people(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Whether this role may hand out `role`, by invitation or by a role change. Only owners make
    /// admins and owners.
    pub fn may_grant(self, role: OrgRole) -> bool {
        match self {
            Self::Owner => true,
            Self::Admin => role == Self::Member,
            Self::Member => false,
        }
    }

    /// Whether this role may change or remove a person holding `target`. Admins act only on
    /// members, and on themselves.
    pub fn may_act_on(self, target: OrgRole, is_self: bool) -> bool {
        match self {
            Self::Owner => true,
            Self::Admin => target == Self::Member || is_self,
            Self::Member => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OrgRole::{Admin, Member, Owner};
    use super::*;

    #[test]
    fn roles_round_trip_and_nothing_else_parses() {
        for role in [Owner, Admin, Member] {
            assert_eq!(OrgRole::parse(role.as_str()), Some(role));
        }
        for junk in ["", "Owner", "superuser", " admin"] {
            assert_eq!(OrgRole::parse(junk), None);
        }
    }

    #[test]
    fn only_owners_make_admins_and_owners() {
        assert!([Owner, Admin, Member]
            .iter()
            .all(|role| Owner.may_grant(*role)));
        assert!(Admin.may_grant(Member));
        assert!(!Admin.may_grant(Admin) && !Admin.may_grant(Owner));
        assert!([Owner, Admin, Member]
            .iter()
            .all(|role| !Member.may_grant(*role)));
    }

    #[test]
    fn admins_act_on_members_and_themselves_only() {
        assert!(Owner.may_act_on(Owner, false));
        assert!(Admin.may_act_on(Member, false));
        assert!(!Admin.may_act_on(Admin, false));
        assert!(!Admin.may_act_on(Owner, false));
        assert!(Admin.may_act_on(Admin, true));
        assert!(!Member.may_act_on(Member, true));
        assert!(Owner.manages_people() && Admin.manages_people() && !Member.manages_people());
    }
}
