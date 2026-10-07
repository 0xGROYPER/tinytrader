//! Push notifications when a service goes down and when it comes back, so
//! the operator hears about it without the console open. Sent as a plain
//! POST to `CONSOLE_NOTIFY` (an ntfy-style URL: the body is the message, with
//! `Title`, `Priority` and `Tags` headers); off when that isn't set.
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
}

const SETTLE: u8 = 2;

impl Watch {
    /// Compare this poll with what was seen, and say what changed.
    pub fn observe(&mut self, t: &Topology, s: &Snapshot) -> Vec<Notice> {
        let mut out = Vec::new();
        for svc in &t.services {
            let Some(st) = s.services.get(&svc.id) else {
                continue;
            };
            let down_now = st.status() == Status::Crit;
            let entry = self.seen.entry(svc.id.clone()).or_insert((down_now, 0));
            if down_now == entry.0 {
                entry.1 = 0;
                continue;
            }
            entry.1 += 1;
            if entry.1 < SETTLE {
                continue;
            }
            *entry = (down_now, 0);
            let name = svc.name();
            out.push(if down_now {
                Notice {
                    title: format!("{name} is down"),
                    body: st.detail(),
                    down: true,
                }
            } else {
                Notice {
                    title: format!("{name} is back"),
                    body: st.detail(),
                    down: false,
                }
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
}
