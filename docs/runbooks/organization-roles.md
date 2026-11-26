# Organization roles

Everyone in an organization is an owner, an admin or a member. Owners manage everyone. Admins
invite members, manage members and may step down or leave. Members see who is in the
organization. Roles are read from the database on every request, so a change holds from the
person's next click.

## Inviting the first person

```sh
edge invite --tenant T --email ada@example.com
```

An operator's invitation without `--role owner|admin|member` grants owner while the organization
has no active person (by any way of signing in) and no pending owner's invitation, and member
after that. It prints the role it chose.

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
