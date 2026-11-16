//! Domain events from the difference between the old and new state of a
//! match. Pure; only real changes produce events.

use domain::{DomainEvent, Match, MatchMap, MatchStatus};

/// Events for an existing match whose fields and maps changed from `old` to
/// `new`. `new_maps` holds only the maps the merge produced; maps missing
/// there are unchanged. New matches only get `MatchCreated` (see
/// [`DomainEvent::MatchCreated`]).
pub fn match_events(
    old: &Match,
    old_maps: &[MatchMap],
    new: &Match,
    new_maps: &[MatchMap],
) -> Vec<DomainEvent> {
    let match_id = new.id;
    let (o, n) = (&old.fields, &new.fields);
    let mut events = Vec::new();

    if o.scheduled_at != n.scheduled_at {
        events.push(DomainEvent::MatchRescheduled {
            match_id,
            from: o.scheduled_at,
            to: n.scheduled_at,
        });
    }
    if o.status != n.status {
        match n.status {
            MatchStatus::Live => events.push(DomainEvent::MatchStarted { match_id }),
            MatchStatus::Finished => events.push(DomainEvent::MatchFinished { match_id }),
            MatchStatus::Canceled => events.push(DomainEvent::MatchCanceled { match_id }),
            MatchStatus::Scheduled | MatchStatus::Postponed => {}
        }
    }

    let mut map_result_changed = false;
    for nm in new_maps {
        let om = old_maps.iter().find(|m| m.number == nm.number);
        if om != Some(nm) {
            events.push(DomainEvent::MapUpdated {
                match_id,
                number: nm.number,
            });
            let result = |m: &MatchMap| (m.status, m.rounds_a, m.rounds_b, m.winner_team_id);
            if om.map(result) != Some(result(nm)) {
                map_result_changed = true;
            }
        }
    }

    let series_changed = (o.maps_won_a, o.maps_won_b, o.winner_team_id)
        != (n.maps_won_a, n.maps_won_b, n.winner_team_id);
    if o.status == MatchStatus::Finished
        && n.status == MatchStatus::Finished
        && (series_changed || map_result_changed)
    {
        events.push(DomainEvent::ResultCorrected { match_id });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use domain::{EventId, MapStatus, MatchFields, MatchFormat, MatchId, TeamId};

    fn base() -> Match {
        let at = Utc.with_ymd_and_hms(2026, 11, 20, 18, 0, 0).unwrap();
        Match {
            id: MatchId(1),
            fields: MatchFields::scheduled(
                EventId(1),
                Some(TeamId(1)),
                Some(TeamId(2)),
                MatchFormat::Bo3,
                Some(at),
            ),
        }
    }

    fn map(number: u8, rounds: Option<(u16, u16)>) -> MatchMap {
        MatchMap {
            match_id: MatchId(1),
            number,
            map_name: None,
            status: if rounds.is_some() {
                MapStatus::Finished
            } else {
                MapStatus::Upcoming
            },
            rounds_a: rounds.map(|r| r.0),
            rounds_b: rounds.map(|r| r.1),
            winner_team_id: None,
        }
    }

    #[test]
    fn no_change_no_events() {
        let m = base();
        let maps = [map(1, Some((13, 5)))];
        assert!(match_events(&m, &maps, &m, &maps).is_empty());
        assert!(
            match_events(&m, &maps, &m, &[]).is_empty(),
            "unreported maps"
        );
    }

    #[test]
    fn reschedule_and_status_transitions() {
        let old = base();
        let mut new = base();
        new.fields.scheduled_at =
            Some(old.fields.scheduled_at.unwrap() + chrono::Duration::hours(1));
        new.fields.status = MatchStatus::Live;
        let ev = match_events(&old, &[], &new, &[]);
        assert_eq!(ev.len(), 2);
        assert!(matches!(ev[0], DomainEvent::MatchRescheduled { .. }));
        assert_eq!(
            ev[1],
            DomainEvent::MatchStarted {
                match_id: MatchId(1)
            }
        );

        for (status, expected) in [
            (
                MatchStatus::Finished,
                Some(DomainEvent::MatchFinished {
                    match_id: MatchId(1),
                }),
            ),
            (
                MatchStatus::Canceled,
                Some(DomainEvent::MatchCanceled {
                    match_id: MatchId(1),
                }),
            ),
            (MatchStatus::Postponed, None),
        ] {
            let mut new = base();
            new.fields.status = status;
            assert_eq!(
                match_events(&old, &[], &new, &[]).first().cloned(),
                expected
            );
        }
    }

    #[test]
    fn map_changes_and_result_corrections() {
        let mut old = base();
        old.fields.status = MatchStatus::Finished;
        old.fields.maps_won_a = 2;
        old.fields.winner_team_id = Some(TeamId(1));
        let old_maps = [map(1, Some((13, 5)))];

        // A new map name only: MapUpdated, but no correction.
        let mut named = map(1, Some((13, 5)));
        named.map_name = Some("Mirage".into());
        assert_eq!(
            match_events(&old, &old_maps, &old, &[named]),
            vec![DomainEvent::MapUpdated {
                match_id: MatchId(1),
                number: 1
            }]
        );

        // Rounds corrected after the finish.
        let ev = match_events(&old, &old_maps, &old, &[map(1, Some((13, 6)))]);
        assert_eq!(
            ev,
            vec![
                DomainEvent::MapUpdated {
                    match_id: MatchId(1),
                    number: 1
                },
                DomainEvent::ResultCorrected {
                    match_id: MatchId(1)
                },
            ]
        );

        // Series corrected.
        let mut new = old.clone();
        new.fields.maps_won_b = 1;
        assert_eq!(
            match_events(&old, &old_maps, &new, &[]),
            vec![DomainEvent::ResultCorrected {
                match_id: MatchId(1)
            }]
        );
    }

    #[test]
    fn finishing_is_not_a_correction() {
        let mut old = base();
        old.fields.status = MatchStatus::Live;
        let mut new = old.clone();
        new.fields.status = MatchStatus::Finished;
        new.fields.maps_won_a = 2;
        let ev = match_events(&old, &[], &new, &[map(2, Some((13, 3)))]);
        assert_eq!(
            ev,
            vec![
                DomainEvent::MatchFinished {
                    match_id: MatchId(1)
                },
                DomainEvent::MapUpdated {
                    match_id: MatchId(1),
                    number: 2
                },
            ]
        );
    }
}
