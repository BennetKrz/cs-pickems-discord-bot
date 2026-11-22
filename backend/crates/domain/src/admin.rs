//! Who counts as an admin of a guild (T07).
//!
//! A member is an admin if they own the guild, if their guild-level
//! permissions include *Manage Server* (or *Administrator*, which implies
//! every permission), or if they have the role configured with
//! `/admin role` (`guild_settings.admin_role_id`).
//!
//! Guild-level permissions follow Discord's "base permissions": the
//! `@everyone` role (its ID equals the guild ID) combined with every role
//! of the member. Channel overwrites play no part, because *Manage Server*
//! is a guild permission. The facts come from Discord (the HTTP API reads
//! them with the bot token, the bot from interactions); this function only
//! decides.

use std::collections::HashMap;

/// `ADMINISTRATOR`: every permission, bypasses channel overwrites.
pub const ADMINISTRATOR: u64 = 1 << 3;
/// `MANAGE_GUILD`, shown as *Manage Server* in the Discord client.
pub const MANAGE_GUILD: u64 = 1 << 5;

/// Parses a permission bit set as Discord serializes it: a decimal string.
/// Returns `None` for anything else.
pub fn parse_permissions(s: &str) -> Option<u64> {
    s.parse().ok()
}

/// What decides whether a member is an admin.
#[derive(Debug, Clone, Copy)]
pub struct AdminFacts<'a> {
    pub guild_id: &'a str,
    pub owner_id: &'a str,
    /// Permissions of every role in the guild, by role ID, including the
    /// `@everyone` role under the guild ID.
    pub role_permissions: &'a HashMap<String, u64>,
    pub user_id: &'a str,
    /// The member's role IDs (without `@everyone`).
    pub member_roles: &'a [String],
    /// The role configured with `/admin role`, if any.
    pub admin_role_id: Option<&'a str>,
}

/// The member's base permissions: `@everyone` plus every role they have.
/// Unknown role IDs (e.g. a role deleted since) count as no permissions.
pub fn base_permissions(f: &AdminFacts<'_>) -> u64 {
    let everyone = f.role_permissions.get(f.guild_id).copied().unwrap_or(0);
    f.member_roles
        .iter()
        .filter_map(|r| f.role_permissions.get(r))
        .fold(everyone, |acc, p| acc | p)
}

/// Whether the member is an admin of the guild.
pub fn is_admin(f: &AdminFacts<'_>) -> bool {
    if f.user_id == f.owner_id {
        return true;
    }
    if base_permissions(f) & (ADMINISTRATOR | MANAGE_GUILD) != 0 {
        return true;
    }
    f.admin_role_id
        .is_some_and(|role| f.member_roles.iter().any(|r| r == role))
}

/// The same rule applied to what an interaction (a slash command) carries
/// about its member (T08): `permissions` are the member's computed
/// permissions in the channel, `None` if Discord sent none.
///
/// This needs no guild data: Discord grants the owner every permission in
/// interactions, *Administrator* implies every permission, and *Manage
/// Server* is a guild-wide permission that channel overwrites cannot add or
/// remove. So the owner and *Manage Server* show up in `permissions`, and
/// the result equals [`is_admin`] on the guild's roles.
pub fn is_admin_from_interaction(
    permissions: Option<u64>,
    member_roles: &[String],
    admin_role_id: Option<&str>,
) -> bool {
    if permissions.is_some_and(|p| p & (ADMINISTRATOR | MANAGE_GUILD) != 0) {
        return true;
    }
    admin_role_id.is_some_and(|role| member_roles.iter().any(|r| r == role))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interaction_rule_table() {
        let roles = |r: &[&str]| r.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        // (name, permissions, member roles, configured role, admin?)
        type Case<'a> = (&'a str, Option<u64>, &'a [&'a str], Option<&'a str>, bool);
        let cases: [Case<'_>; 8] = [
            ("owner: every permission", Some(u64::MAX), &[], None, true),
            (
                "Administrator",
                Some(ADMINISTRATOR | 0x400),
                &["10"],
                None,
                true,
            ),
            (
                "Manage Server",
                Some(MANAGE_GUILD | 0x800),
                &["11"],
                None,
                true,
            ),
            (
                "configured role",
                Some(0x400),
                &["10", "13"],
                Some("13"),
                true,
            ),
            ("normal member", Some(0x400 | 0x800), &["10"], None, false),
            (
                "lacks the configured role",
                Some(0x400),
                &["10"],
                Some("13"),
                false,
            ),
            (
                "deleted configured role",
                Some(0x400),
                &["10"],
                Some("999"),
                false,
            ),
            ("no permissions sent", None, &["10"], None, false),
        ];
        for (name, perms, member_roles, admin_role, admin) in cases {
            assert_eq!(
                is_admin_from_interaction(perms, &roles(member_roles), admin_role),
                admin,
                "{name}"
            );
        }
    }

    const GUILD: &str = "100";
    const OWNER: &str = "1";
    const USER: &str = "2";

    fn roles(everyone: u64) -> HashMap<String, u64> {
        HashMap::from([
            (GUILD.to_owned(), everyone),
            ("10".to_owned(), 0x400 | 0x800), // view channel, send messages
            ("11".to_owned(), MANAGE_GUILD),
            ("12".to_owned(), ADMINISTRATOR),
            ("13".to_owned(), 0), // the configured pick'em admin role
        ])
    }

    #[test]
    fn admin_rule_table() {
        struct Case {
            name: &'static str,
            user: &'static str,
            everyone: u64,
            member_roles: &'static [&'static str],
            admin_role: Option<&'static str>,
            admin: bool,
        }
        let cases = [
            Case {
                name: "owner without roles",
                user: OWNER,
                everyone: 0,
                member_roles: &[],
                admin_role: None,
                admin: true,
            },
            Case {
                name: "role with Manage Server",
                user: USER,
                everyone: 0,
                member_roles: &["10", "11"],
                admin_role: None,
                admin: true,
            },
            Case {
                name: "role with Administrator",
                user: USER,
                everyone: 0,
                member_roles: &["12"],
                admin_role: None,
                admin: true,
            },
            Case {
                name: "Manage Server on @everyone",
                user: USER,
                everyone: MANAGE_GUILD,
                member_roles: &[],
                admin_role: None,
                admin: true,
            },
            Case {
                name: "configured admin role",
                user: USER,
                everyone: 0,
                member_roles: &["10", "13"],
                admin_role: Some("13"),
                admin: true,
            },
            Case {
                name: "normal member",
                user: USER,
                everyone: 0x400,
                member_roles: &["10"],
                admin_role: None,
                admin: false,
            },
            Case {
                name: "configured role the member lacks",
                user: USER,
                everyone: 0,
                member_roles: &["10"],
                admin_role: Some("13"),
                admin: false,
            },
            Case {
                name: "unknown (deleted) role grants nothing",
                user: USER,
                everyone: 0,
                member_roles: &["999"],
                admin_role: None,
                admin: false,
            },
        ];
        for c in cases {
            let role_permissions = roles(c.everyone);
            let member_roles: Vec<String> =
                c.member_roles.iter().map(|r| (*r).to_owned()).collect();
            let facts = AdminFacts {
                guild_id: GUILD,
                owner_id: OWNER,
                role_permissions: &role_permissions,
                user_id: c.user,
                member_roles: &member_roles,
                admin_role_id: c.admin_role,
            };
            assert_eq!(is_admin(&facts), c.admin, "{}", c.name);
        }
    }

    #[test]
    fn base_permissions_combine_everyone_and_member_roles() {
        let role_permissions = roles(0x1);
        let member_roles = vec!["10".to_owned(), "11".to_owned()];
        let facts = AdminFacts {
            guild_id: GUILD,
            owner_id: OWNER,
            role_permissions: &role_permissions,
            user_id: USER,
            member_roles: &member_roles,
            admin_role_id: None,
        };
        assert_eq!(base_permissions(&facts), 0x1 | 0x400 | 0x800 | MANAGE_GUILD);
    }

    #[test]
    fn permissions_parse_from_decimal_strings() {
        assert_eq!(parse_permissions("32"), Some(MANAGE_GUILD));
        assert_eq!(parse_permissions("66321471"), Some(66_321_471));
        // Bits beyond 32 occur in practice.
        assert_eq!(parse_permissions("2251799813685247"), Some((1 << 51) - 1));
        assert_eq!(parse_permissions(""), None);
        assert_eq!(parse_permissions("0x20"), None);
        assert_eq!(parse_permissions("-1"), None);
    }
}
