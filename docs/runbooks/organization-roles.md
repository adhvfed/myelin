# Organization roles

Everyone in an organization is an owner, an admin or a member. Owners manage everyone. Admins
invite members, manage members and may step down or leave. Members see who is in the
organization. Roles are read from the database on every request, so a change holds from the
person's next click.

## Inviting the first person

```sh
edge invite --tenant T --email ada@example.com
```

An organization's first person joins as owner. Once anyone is in it, an operator's invitation
grants member unless it names a role with `--role owner|admin|member`.

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

Removing someone disables them, ends every session they hold, and withdraws the invitations they
sent. Their address can be invited again; accepting sets a new password and brings back the same
person, whose old sessions stay ended. An erased person can never be invited back.
