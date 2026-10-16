//! Push notifications when a service goes down and when it comes back, so
//! the operator hears about it without the console open. Sent as a plain
//! POST to `CONSOLE_NOTIFY` (an ntfy-style URL: the body is the message, with
//! `Title`, `Priority` and `Tags` headers); off when that isn't set.
//!
//! Down is [`Status::Crit`]: a container exited, dead or restarting, or a
//! service reporting `failed`. A container that disappears (it was seen, and
//! is no longer listed) counts as down too, until it is back.
//!
//! A service counts as down after two polls in a row (about 4 s), so a
//! restart doesn't page anyone, and as back after two polls up.

use std::collections::HashMap;

use tinytrader_fleet::{Snapshot, Status, Topology};

/// What to say, if anything changed.
#[derive(Debug, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    pub down: bool,
}

#[derive(Default)]
pub struct Watch {
    /// Per service: whether it is taken to be down, and polls seen the other way.
    seen: HashMap<String, (bool, u8)>,
    /// Services that have had a container, with the name to show: if one
    /// goes missing, its container is gone.
    had_container: HashMap<String, String>,
}

const SETTLE: u8 = 2;

impl Watch {
    /// Compare this poll with what was seen, and say what changed.
    pub fn observe(&mut self, t: &Topology, s: &Snapshot) -> Vec<Notice> {
        let mut out = Vec::new();
        // Each service in this poll, and each that had a container and isn't
        // in it any more: (id, name, down now, what to say).
        let mut now: Vec<(String, String, bool, String)> = Vec::new();
        for svc in &t.services {
            let name = svc.name().to_owned();
            match s.services.get(&svc.id) {
                Some(st) if st.container.is_some() => {
                    self.had_container.insert(svc.id.clone(), name.clone());
                    now.push((
                        svc.id.clone(),
                        name,
                        st.status() == Status::Crit,
                        st.detail(),
                    ));
                }
                Some(st) if !self.had_container.contains_key(&svc.id) => {
                    now.push((
                        svc.id.clone(),
                        name,
                        st.status() == Status::Crit,
                        st.detail(),
                    ));
                }
                // Listed, but its container is gone.
                Some(_) => now.push((svc.id.clone(), name, true, "its container is gone".into())),
                None => {}
            }
        }
        for (id, name) in &self.had_container {
            if !now.iter().any(|(i, ..)| i == id) {
                now.push((
                    id.clone(),
                    name.clone(),
                    true,
                    "its container is gone".into(),
                ));
            }
        }
        for (id, name, down_now, detail) in now {
            let entry = self.seen.entry(id).or_insert((down_now, 0));
            if down_now == entry.0 {
                entry.1 = 0;
                continue;
            }
            entry.1 += 1;
            if entry.1 < SETTLE {
                continue;
            }
            *entry = (down_now, 0);
            out.push(Notice {
                title: format!("{name} is {}", if down_now { "down" } else { "back" }),
                body: detail,
                down: down_now,
            });
        }
        out
    }
}

/// Send a notice; failures are returned for the console's own health.
pub async fn send(http: &reqwest::Client, url: &str, n: &Notice) -> Result<(), String> {
    let r = http
        .post(url)
        .header("Title", n.title.clone())
        .header("Priority", if n.down { "high" } else { "default" })
        .header(
            "Tags",
            if n.down {
                "rotating_light"
            } else {
                "white_check_mark"
            },
        )
        .body(if n.body.is_empty() {
            n.title.clone()
        } else {
            n.body.clone()
        })
        .send()
        .await
        .map_err(|e| format!("notify: {e}"))?;
    if r.status().is_success() {
        Ok(())
    } else {
        Err(format!("notify: {}", r.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn world(state: &str) -> (Topology, Snapshot) {
        let t: Topology = serde_json::from_value(
            json!({ "services": [{ "id": "relay", "label": "relay SPY" }] }),
        )
        .unwrap();
        let s: Snapshot = serde_json::from_value(json!({ "services": { "relay": { "container": { "state": state, "status": state } } } })).unwrap();
        (t, s)
    }

    #[test]
    fn says_down_and_back_once_each_after_two_polls() {
        let mut w = Watch::default();
        let (t, up) = world("running");
        let (_, down) = world("exited");
        assert!(w.observe(&t, &up).is_empty(), "first sight is not news");
        assert!(
            w.observe(&t, &down).is_empty(),
            "one poll down could be a restart"
        );
        let n = w.observe(&t, &down);
        assert_eq!(n.len(), 1);
        assert!(n[0].down && n[0].title == "relay SPY is down");
        assert!(w.observe(&t, &down).is_empty(), "said once");
        assert!(w.observe(&t, &up).is_empty());
        let n = w.observe(&t, &up);
        assert_eq!(n[0].title, "relay SPY is back");
        // A blip of one poll says nothing.
        assert!(w.observe(&t, &down).is_empty());
        assert!(w.observe(&t, &up).is_empty());
        assert!(w.observe(&t, &up).is_empty());
    }

    #[test]
    fn a_restart_loop_and_a_failed_service_are_down() {
        let mut w = Watch::default();
        let (t, up) = world("running");
        let (_, looping) = world("restarting");
        w.observe(&t, &up);
        w.observe(&t, &looping);
        let n = w.observe(&t, &looping);
        assert!(n.len() == 1 && n[0].down, "{n:?}");

        let mut w = Watch::default();
        let failed: Snapshot = serde_json::from_value(json!({ "services": { "relay": {
            "container": { "state": "running" },
            "health": { "http_status": 503, "body": { "state": "failed", "detail": "feed lost" } }
        } } }))
        .unwrap();
        w.observe(&t, &up);
        w.observe(&t, &failed);
        let n = w.observe(&t, &failed);
        assert_eq!(
            n,
            vec![Notice {
                title: "relay SPY is down".into(),
                body: "feed lost".into(),
                down: true
            }]
        );
    }

    #[test]
    fn a_container_that_disappears_is_down_until_it_is_back() {
        let mut w = Watch::default();
        let (t, up) = world("running");
        // Gone from the topology and the state alike (the console lists
        // services from Docker's containers).
        let (empty_t, empty_s) = (Topology::default(), Snapshot::default());
        w.observe(&t, &up);
        assert!(
            w.observe(&empty_t, &empty_s).is_empty(),
            "one poll could be a recreate"
        );
        let n = w.observe(&empty_t, &empty_s);
        assert_eq!(n.len(), 1);
        assert!(n[0].down && n[0].title == "relay SPY is down" && n[0].body.contains("gone"));
        assert!(w.observe(&empty_t, &empty_s).is_empty(), "said once");
        // Still listed (by the topology file), but with no container.
        let listed: Snapshot =
            serde_json::from_value(json!({ "services": { "relay": { "container": null } } }))
                .unwrap();
        assert!(w.observe(&t, &listed).is_empty(), "still down");
        w.observe(&t, &up);
        let n = w.observe(&t, &up);
        assert!(n.len() == 1 && !n[0].down && n[0].title == "relay SPY is back");
    }

    #[test]
    fn a_service_without_a_container_is_judged_by_health() {
        let mut w = Watch::default();
        let t: Topology =
            serde_json::from_value(json!({ "services": [{ "id": "store" }] })).unwrap();
        let ok: Snapshot = serde_json::from_value(
            json!({ "services": { "store": { "health": { "http_status": 200, "body": { "state": "ready" } } } } }),
        )
        .unwrap();
        for _ in 0..3 {
            assert!(w.observe(&t, &ok).is_empty());
        }
    }
}
