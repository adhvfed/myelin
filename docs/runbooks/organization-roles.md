# Organization roles

Everyone in an organization is an owner, an admin or a member. Owners manage everyone. Admins
invite members, manage members and may step down or leave. Members see who is in the
organization. Roles are read from the database on every request, so a change holds from the
person's next click.

## Inviting the first person

```sh
edge invite --tenant T --email ada@example.com
```

An operator's invitation without `--role owner|admin|member` grants owner while nobody active in
the organization holds a role or signs in with SSO and no owner's invitation is pending, and member
after that. Operator principals made by `edge bootstrap` are not people of the organization and do
not count, so the documented order (bootstrap, then invite) still makes the first person owner. The
command prints the role it chose.

## Recovering an organization with no owner

Nobody can demote or remove an organization's last owner through Myelin. An erasure or a
principal-wide revocation is never blocked, though, so it can leave an organization with no
active owner. The members page then says so, and the edge logs
`edge.accounts.organization_without_owner` whenever the roster is read.

Give someone the owner role:

```sh
edge role --tenant T --email grace@example.com --role owner
edge role --tenant T --principal person-01... --role owner
```

`--principal` also works for a person who signs in without a password (SSO, or a development
login) and has no role yet: they are given the role. `edge role` never demotes the last owner;
promote another owner first.

## Removed people

Removing someone disables them for good, ends every session they hold, and is recorded as a
removal. Their address can be invited again: accepting makes a new person with the name and
password they choose, and moves the address's sign-in to them. The removed person stays disabled
with their history, agents, tokens and grants, none of which the new person inherits. A person
disabled any other way, or erased, keeps their address; it cannot be invited.

Whenever a person stops being active (removed, erased or disabled), the invitations they sent stop
working and their unsent mail is withdrawn.

`edge bootstrap --principal` refuses a principal that was removed, as it refuses an erased one:
making it active again would hand back everything tied to it. Invite their address instead.

Erasing a person who came back this way erases every principal they were: the successions are kept
in `principal_succession`.

### Single sign-on after returning

A person who signs in with SSO keeps their SSO link on the principal that was removed, so after
they come back their SSO sign-in is refused until an operator links their SSO identity to the new
principal. Myelin has no command for that yet; until it does, the person signs in with the
password they chose when they came back.
