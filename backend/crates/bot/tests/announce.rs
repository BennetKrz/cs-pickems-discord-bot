//! The announcer against the database (T08): the fixture lifecycle (T05)
//! produces each digest and result post once, also across restarts; the
//! rate limit, bundling and channel rules hold. Discord is replaced by a
//! recorder that deduplicates nonces like Discord does.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bot::announce::{Announcer, Outbox, OutboxError, TickReport};
use bot::render::Post;
use chrono::{DateTime, Duration, TimeZone, Utc};
use domain::{
    Clock, EntityType, EventId, EventStatus, EventTier, FakeClock, MatchFields, MatchFormat,
    MatchId, MatchStatus, NewEvent, NewTeam, PickParts, TeamId,
};
use sources::SourceAdapter;
use sources::fixture::{FIXTURE, FixtureAdapter, Scenario};
use store::SqlitePool;
use store::bot::PostKind;
use store::guilds::FollowedEvent;
use store::users::User;
use sync::{EventBus, Precedence, Reconciler};

const GUILD: &str = "100000000000000001";
const CHANNEL: &str = "300000000000000001";
const ALICE: &str = "200000000000000001";
const BOB: &str = "200000000000000002";

// ---- a Discord stand-in -----------------------------------------------------------

#[derive(Debug, Clone)]
struct Sent {
    channel: String,
    message_id: String,
    nonce: String,
    post: Post,
    at: DateTime<Utc>,
}

/// Records sends and edits. Like Discord with `enforce_nonce`, a repeated
/// nonce returns the existing message instead of creating one.
struct Recorder {
    clock: Arc<FakeClock>,
    state: Mutex<RecorderState>,
}

#[derive(Default)]
struct RecorderState {
    created: Vec<Sent>,
    /// Sends that returned an existing message (deduplicated by nonce).
    deduplicated: u32,
    edits: Vec<(String, Post)>,
    /// Fail this many upcoming sends *after* Discord created the message
    /// (as if the connection dropped before the answer arrived).
    lose_answers: u32,
}

impl Recorder {
    fn new(clock: Arc<FakeClock>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            state: Mutex::new(RecorderState::default()),
        })
    }

    fn created(&self) -> Vec<Sent> {
        self.state.lock().unwrap().created.clone()
    }

    fn edits(&self) -> Vec<(String, Post)> {
        self.state.lock().unwrap().edits.clone()
    }
}

#[async_trait]
impl Outbox for Recorder {
    async fn send(
        &self,
        channel_id: &str,
        post: &Post,
        nonce: &str,
    ) -> Result<String, OutboxError> {
        let mut s = self.state.lock().unwrap();
        let existing = s
            .created
            .iter()
            .find(|m| m.nonce == nonce)
            .map(|m| m.message_id.clone());
        let id = match existing {
            Some(id) => {
                s.deduplicated += 1;
                id
            }
            None => {
                let id = (900_000_000_000_000_000u64 + s.created.len() as u64).to_string();
                s.created.push(Sent {
                    channel: channel_id.to_owned(),
                    message_id: id.clone(),
                    nonce: nonce.to_owned(),
                    post: post.clone(),
                    at: self.clock.now(),
                });
                id
            }
        };
        if s.lose_answers > 0 {
            s.lose_answers -= 1;
            return Err(OutboxError::Failed("connection reset".into()));
        }
        Ok(id)
    }

    async fn edit(
        &self,
        _channel_id: &str,
        message_id: &str,
        post: &Post,
    ) -> Result<(), OutboxError> {
        self.state
            .lock()
            .unwrap()
            .edits
            .push((message_id.to_owned(), post.clone()));
        Ok(())
    }
}

// ---- the fixture lifecycle ----------------------------------------------------------

/// 09:30 UTC = 10:30 in Berlin: past the default digest time (10:00).
/// The Berlin day ends at anchor + 810 min.
fn anchor() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 20, 9, 30, 0).unwrap()
}

struct Env {
    pool: SqlitePool,
    rec: Reconciler,
    clock: Arc<FakeClock>,
    src: FixtureAdapter,
    outbox: Arc<Recorder>,
    _dir: tempfile::TempDir,
}

async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}", dir.path().join("test.db").display());
    (store::open(&url).await.unwrap(), dir)
}

async fn user(pool: &SqlitePool, id: &str, name: &str) {
    store::users::upsert_user(
        pool,
        &User {
            discord_user_id: id.into(),
            username: name.to_lowercase(),
            global_name: Some(name.into()),
            avatar_hash: None,
            updated_at: anchor(),
        },
    )
    .await
    .unwrap();
}

impl Env {
    async fn new() -> Self {
        let (pool, dir) = temp_pool().await;
        let rec = Reconciler::new(
            pool.clone(),
            Precedence::from_enabled(&[FIXTURE.to_owned()]),
            Some(FIXTURE.to_owned()),
            EventBus::new(),
        );
        let clock = Arc::new(FakeClock::new(anchor()));
        let src = FixtureAdapter::new(Scenario::default_scenario(), anchor(), clock.clone());
        let outbox = Recorder::new(clock.clone());
        let env = Self {
            pool,
            rec,
            clock,
            src,
            outbox,
            _dir: dir,
        };
        env.sync().await;
        let event = store::refs::resolve(&env.pool, FIXTURE, EntityType::Event, "fx-major")
            .await
            .unwrap()
            .unwrap();
        store::guilds::follow_event(
            &env.pool,
            &FollowedEvent {
                guild_id: GUILD.into(),
                event_id: EventId(event),
                followed_by: ALICE.into(),
                followed_at: anchor(),
            },
        )
        .await
        .unwrap();
        store::guilds::set_announce_channel(&env.pool, GUILD, CHANNEL, anchor())
            .await
            .unwrap();
        user(&env.pool, ALICE, "Alice").await;
        user(&env.pool, BOB, "Bob").await;
        env
    }

    fn announcer(&self) -> Announcer {
        Announcer::new(
            self.pool.clone(),
            self.clock.clone(),
            self.outbox.clone(),
            Some(GUILD.into()),
        )
    }

    /// Syncs the fixture event at the current clock, then locks and
    /// settles like the lock ticker and the settler.
    async fn sync(&self) {
        let now = self.clock.now();
        let ev = self.src.fetch_event("fx-major").await.unwrap();
        self.rec.ingest_event(FIXTURE, &ev, now).await.unwrap();
        let ms = self.src.fetch_event_matches("fx-major").await.unwrap();
        let rep = self.rec.ingest_matches(FIXTURE, &ms, now).await.unwrap();
        assert_eq!(rep.failed, 0);
        store::matches::lock_due(&self.pool, now).await.unwrap();
        sync::settle::sweep(&self.pool, now).await.unwrap();
    }

    async fn at(&self, minutes: i64) {
        self.clock.set(anchor() + Duration::minutes(minutes));
        self.sync().await;
    }

    async fn id(&self, ext: &str) -> MatchId {
        MatchId(
            store::refs::resolve(&self.pool, FIXTURE, EntityType::Match, ext)
                .await
                .unwrap()
                .unwrap(),
        )
    }

    async fn pick_winner(&self, user: &str, ext: &str, team_a: bool) {
        let id = self.id(ext).await;
        let m = store::matches::get(&self.pool, id).await.unwrap().unwrap();
        let team = if team_a {
            m.fields.team_a_id
        } else {
            m.fields.team_b_id
        };
        let parts = PickParts {
            winner_team_id: team,
            series: None,
            maps: Vec::new(),
        };
        store::picks::save(&self.pool, GUILD, user, id, &parts, self.clock.now())
            .await
            .unwrap();
    }
}

fn digests(sent: &[Sent]) -> Vec<&Sent> {
    sent.iter()
        .filter(|s| s.post.title.starts_with("Today's"))
        .collect()
}

fn result_posts(sent: &[Sent]) -> Vec<&Sent> {
    sent.iter()
        .filter(|s| s.post.title.starts_with("Result"))
        .collect()
}

/// The matches of every result post, from the database, by post.
async fn announced(pool: &SqlitePool) -> Vec<Vec<MatchId>> {
    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM bot_posts WHERE kind = 'results' ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
    let mut out = Vec::new();
    for id in ids {
        out.push(store::bot::post_matches(pool, id).await.unwrap());
    }
    out
}

fn max_posts_in_an_hour(sent: &[Sent]) -> usize {
    sent.iter()
        .map(|s| {
            sent.iter()
                .filter(|o| o.at >= s.at && o.at < s.at + Duration::hours(1))
                .count()
        })
        .max()
        .unwrap_or(0)
}

#[tokio::test]
async fn fixture_lifecycle_posts_each_digest_and_result_once_across_restarts() {
    let env = Env::new().await;
    // Picks before the first match locks (+30).
    env.pick_winner(ALICE, "sw-r1-1", true).await; // right: team A wins 13-9
    env.pick_winner(BOB, "sw-r1-1", false).await;

    let mut announcer = env.announcer();
    let r = announcer.tick().await.unwrap();
    assert_eq!(r.sent, 1, "the digest goes out at once (10:30 > 10:00)");
    let sent = env.outbox.created();
    assert_eq!(digests(&sent).len(), 1);
    let digest = &sent[0];
    assert_eq!(digest.channel, CHANNEL);
    assert!(
        digest.post.title.contains("Fri 20 Nov"),
        "{}",
        digest.post.title
    );
    assert!(digest.post.description.contains("2 picks"));

    // A pick changes the count: the digest is edited, not re-posted, and
    // only once the minute since the post has passed.
    env.pick_winner(ALICE, "sw-r1-2", true).await;
    assert_eq!(announcer.tick().await.unwrap().edited, 0, "too soon");
    env.clock.set(anchor() + Duration::minutes(2));
    assert_eq!(announcer.tick().await.unwrap().edited, 1);
    assert_eq!(announcer.tick().await.unwrap(), TickReport::default());
    let (edited_id, edited) = env.outbox.edits().last().cloned().unwrap();
    assert_eq!(edited_id, digest.message_id);
    assert!(edited.description.contains("1 pick"));

    // Walk the whole scenario in 15-minute steps, restarting the
    // announcer every 6 hours.
    let mut minute = 0;
    while minute <= 2_000 {
        env.at(minute).await;
        announcer.tick().await.unwrap();
        if minute % 360 == 0 {
            announcer = env.announcer();
            announcer.tick().await.unwrap();
        }
        minute += 15;
    }
    // And once more after a restart at the end: nothing new.
    let mut announcer = env.announcer();
    assert_eq!(announcer.tick().await.unwrap(), TickReport::default());

    let sent = env.outbox.created();
    assert_eq!(
        digests(&sent).len(),
        1,
        "one digest; tomorrow nothing is pickable at 10:00"
    );
    assert_eq!(env.outbox.state.lock().unwrap().deduplicated, 0);

    // Every finished match is announced exactly once.
    let mut finished = BTreeSet::new();
    for ext in [
        "sw-r1-1", "sw-r1-2", "sw-r1-3", "sw-r2-1", "sw-r2-2", "ub-sf1", "ub-sf2", "ub-f", "lb-r1",
        "lb-f", "gf",
    ] {
        finished.insert(env.id(ext).await);
    }
    let bundles = announced(&env.pool).await;
    let all: Vec<MatchId> = bundles.iter().flatten().copied().collect();
    let unique: BTreeSet<MatchId> = all.iter().copied().collect();
    assert_eq!(all.len(), unique.len(), "no match twice: {bundles:?}");
    assert_eq!(unique, finished, "the canceled match is not announced");
    assert_eq!(
        result_posts(&sent).len(),
        bundles.len(),
        "one message per bundle"
    );
    // Matches that start together finish together and share a post.
    let r1 = [env.id("sw-r1-1").await, env.id("sw-r1-2").await];
    assert!(
        bundles.iter().any(|b| r1.iter().all(|m| b.contains(m))),
        "{bundles:?}"
    );
    assert!(max_posts_in_an_hour(&sent) <= 6);

    // The first round's post names the best picker with points.
    let first = result_posts(&sent)[0];
    let text: String = first
        .post
        .fields
        .iter()
        .map(|f| format!("{}\n{}\n", f.name, f.value))
        .collect();
    assert!(text.contains("Best picks: Alice ("), "{text}");
    assert!(!text.contains("Bob ("), "Bob picked the loser: {text}");
    assert!(
        first
            .post
            .buttons
            .iter()
            .any(|b| b.custom_id == format!("open:{}", r1[0].0))
    );
}

#[tokio::test]
async fn an_interrupted_send_is_resent_with_the_same_nonce() {
    let env = Env::new().await;
    // Discord creates the digest but the answer is lost.
    env.outbox.state.lock().unwrap().lose_answers = 1;
    let mut announcer = env.announcer();
    let r = announcer.tick().await.unwrap();
    assert_eq!(r.failed, 1);
    let claimed = store::bot::unsent(&env.pool, GUILD).await.unwrap();
    assert_eq!(claimed.len(), 1, "claimed, but no message ID stored");

    // A restart (the backoff is per announcer) resends it.
    let mut announcer = env.announcer();
    let r = announcer.tick().await.unwrap();
    assert_eq!(r.resent, 1);
    assert_eq!(env.outbox.created().len(), 1, "Discord deduplicated it");
    assert_eq!(env.outbox.state.lock().unwrap().deduplicated, 1);
    assert!(
        store::bot::unsent(&env.pool, GUILD)
            .await
            .unwrap()
            .is_empty()
    );
    let post = store::bot::get_post(&env.pool, GUILD, PostKind::Digest, "2026-11-20")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        post.message_id.as_deref(),
        Some(env.outbox.created()[0].message_id.as_str())
    );
}

// ---- hand-made matches --------------------------------------------------------------

struct Plain {
    pool: SqlitePool,
    clock: Arc<FakeClock>,
    outbox: Arc<Recorder>,
    event: EventId,
    a: TeamId,
    b: TeamId,
    _dir: tempfile::TempDir,
}

fn noon() -> DateTime<Utc> {
    // 13:00 in Berlin.
    Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap()
}

impl Plain {
    async fn new() -> Self {
        let (pool, dir) = temp_pool().await;
        let event = store::events::insert(
            &pool,
            &NewEvent {
                name: "Cup".into(),
                tier: EventTier::A,
                begin_at: None,
                end_at: None,
                status: EventStatus::Running,
            },
        )
        .await
        .unwrap();
        let team = |name: &'static str| {
            let pool = pool.clone();
            async move {
                store::teams::insert(
                    &pool,
                    &NewTeam {
                        name: name.into(),
                        acronym: None,
                        logo_url: None,
                        country: None,
                    },
                )
                .await
                .unwrap()
            }
        };
        let (a, b) = (team("Alpha").await, team("Bravo").await);
        store::guilds::follow_event(
            &pool,
            &FollowedEvent {
                guild_id: GUILD.into(),
                event_id: event,
                followed_by: ALICE.into(),
                followed_at: noon() - Duration::days(1),
            },
        )
        .await
        .unwrap();
        let clock = Arc::new(FakeClock::new(noon()));
        let outbox = Recorder::new(clock.clone());
        Self {
            pool,
            clock,
            outbox,
            event,
            a,
            b,
            _dir: dir,
        }
    }

    fn announcer(&self) -> Announcer {
        Announcer::new(
            self.pool.clone(),
            self.clock.clone(),
            self.outbox.clone(),
            None,
        )
    }

    async fn finished(&self, ended: DateTime<Utc>) -> MatchId {
        let mut f = MatchFields::scheduled(
            self.event,
            Some(self.a),
            Some(self.b),
            MatchFormat::Bo1,
            Some(ended - Duration::hours(1)),
        );
        f.status = MatchStatus::Finished;
        f.started_at = f.scheduled_at;
        f.ended_at = Some(ended);
        f.locked = true;
        f.maps_won_a = 1;
        f.winner_team_id = Some(self.a);
        store::matches::insert(&self.pool, &f).await.unwrap()
    }

    async fn upcoming(&self, at: DateTime<Utc>) -> MatchId {
        let f = MatchFields::scheduled(
            self.event,
            Some(self.a),
            Some(self.b),
            MatchFormat::Bo3,
            Some(at),
        );
        store::matches::insert(&self.pool, &f).await.unwrap()
    }

    async fn channel_since(&self, since: DateTime<Utc>) {
        store::guilds::set_announce_channel(&self.pool, GUILD, CHANNEL, since)
            .await
            .unwrap();
    }

    fn advance(&self, by: Duration) {
        self.clock.set(self.clock.now() + by);
    }
}

#[tokio::test]
async fn at_most_six_posts_per_hour_then_the_next_free_slot() {
    let p = Plain::new().await;
    p.channel_since(noon() - Duration::days(1)).await;
    // Seven results, each in its own bundle (20 min apart), all long due.
    for i in 0..7 {
        p.finished(noon() - Duration::hours(4) + Duration::minutes(20 * i))
            .await;
    }
    let mut a = p.announcer();
    let mut sent = 0;
    for _ in 0..10 {
        let r = a.tick().await.unwrap();
        sent += r.sent;
        p.advance(Duration::minutes(1));
    }
    assert_eq!(sent, 6, "six posts in the first ten minutes");
    let r = a.tick().await.unwrap();
    assert_eq!((r.sent, r.deferred), (0, 1));

    // A restart doesn't reset the count: it comes from the database.
    let mut a = p.announcer();
    assert_eq!(a.tick().await.unwrap().deferred, 1);

    // An hour after the first post, a slot is free.
    p.clock.set(noon() + Duration::hours(1));
    assert_eq!(a.tick().await.unwrap().sent, 1);
    assert_eq!(result_posts(&p.outbox.created()).len(), 7);
}

#[tokio::test]
async fn results_close_together_share_a_post_after_ten_minutes() {
    let p = Plain::new().await;
    p.channel_since(noon() - Duration::hours(1)).await;
    let m1 = p.finished(noon()).await;
    let m2 = p.finished(noon() + Duration::minutes(8)).await;
    let m3 = p.finished(noon() + Duration::minutes(12)).await;
    let mut a = p.announcer();

    p.clock.set(noon() + Duration::minutes(9));
    assert_eq!(a.tick().await.unwrap().sent, 0, "the window is still open");
    p.clock.set(noon() + Duration::minutes(10));
    assert_eq!(a.tick().await.unwrap().sent, 1);
    assert_eq!(announced(&p.pool).await, vec![vec![m1, m2]]);

    p.clock.set(noon() + Duration::minutes(21));
    assert_eq!(a.tick().await.unwrap().sent, 0);
    p.clock.set(noon() + Duration::minutes(22));
    assert_eq!(a.tick().await.unwrap().sent, 1);
    assert_eq!(announced(&p.pool).await, vec![vec![m1, m2], vec![m3]]);
    let posts = result_posts(&p.outbox.created()).len();
    assert_eq!(posts, 2);
    assert_eq!(p.outbox.created()[0].post.title, "Results");
    assert_eq!(p.outbox.created()[1].post.title, "Result");
}

#[tokio::test]
async fn nothing_is_posted_without_a_channel() {
    let p = Plain::new().await;
    p.finished(noon() - Duration::hours(1)).await;
    p.upcoming(noon() + Duration::hours(3)).await;
    let r = p.announcer().tick().await.unwrap();
    assert_eq!(r, TickReport::default());
    assert!(p.outbox.created().is_empty());
}

#[tokio::test]
async fn results_from_before_the_channel_was_set_are_not_posted() {
    let p = Plain::new().await;
    p.finished(noon() - Duration::hours(2)).await;
    p.channel_since(noon() - Duration::hours(1)).await;
    let after = p.finished(noon() - Duration::minutes(30)).await;
    p.announcer().tick().await.unwrap();
    assert_eq!(announced(&p.pool).await, vec![vec![after]]);
}

#[tokio::test]
async fn the_digest_waits_for_its_time_and_skips_days_without_open_matches() {
    let p = Plain::new().await;
    p.channel_since(noon() - Duration::days(1)).await;
    // 08:30 in Berlin, before the digest time.
    p.clock
        .set(Utc.with_ymd_and_hms(2026, 11, 20, 7, 30, 0).unwrap());
    let mut a = p.announcer();
    assert_eq!(a.tick().await.unwrap(), TickReport::default());

    // 10:00 in Berlin, but no match today: no digest.
    p.clock
        .set(Utc.with_ymd_and_hms(2026, 11, 20, 9, 0, 0).unwrap());
    assert_eq!(a.tick().await.unwrap(), TickReport::default());

    // A match tomorrow doesn't count for today.
    let tomorrow = Utc.with_ymd_and_hms(2026, 11, 21, 18, 0, 0).unwrap();
    p.upcoming(tomorrow).await;
    assert_eq!(a.tick().await.unwrap(), TickReport::default());

    // A match tonight (23:30 Berlin, still today) is synced: the digest
    // goes out.
    p.upcoming(Utc.with_ymd_and_hms(2026, 11, 20, 22, 30, 0).unwrap())
        .await;
    assert_eq!(a.tick().await.unwrap().sent, 1);
    assert_eq!(digests(&p.outbox.created()).len(), 1);

    // Another guild time zone and time move the schedule.
    store::guilds::set_digest_schedule(&p.pool, GUILD, "18:30", "America/New_York", noon())
        .await
        .unwrap();
    // 18:30 in New York on the 21st is 23:30 UTC.
    p.clock
        .set(Utc.with_ymd_and_hms(2026, 11, 21, 23, 29, 0).unwrap());
    assert_eq!(a.tick().await.unwrap().sent, 0);
    // By then tomorrow's match (18:00 UTC) has locked: nothing pickable.
    p.clock
        .set(Utc.with_ymd_and_hms(2026, 11, 21, 23, 30, 0).unwrap());
    assert_eq!(a.tick().await.unwrap().sent, 0);
    let keys: BTreeMap<String, u32> = sqlx::query_as::<_, (String, u32)>(
        "SELECT post_key, COUNT(*) FROM bot_posts WHERE kind = 'digest' GROUP BY post_key",
    )
    .fetch_all(&p.pool)
    .await
    .unwrap()
    .into_iter()
    .collect();
    assert_eq!(keys, BTreeMap::from([("2026-11-20".to_owned(), 1)]));
}
