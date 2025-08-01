use hanabot::{Hanabi, MessageProxy};
use slack_morphism::SlackUserId;
use std::collections::HashMap;

// TODO: test save?
// TODO: insta
// TODO: test auto-start at 5
// TODO: ensure games are deterministically random

#[tokio::test]
async fn help() {
    let mut hanabi = Hanabi::default();
    let mut out = DummyMessageProxy::default();
    hanabi
        .on_dm_recv("help", SlackUserId("a".to_string()), &mut out)
        .await
        .unwrap();

    assert!(out.msgs.contains_key("a"));
    assert_eq!(out.msgs.len(), 1);
    assert!(out.msgs["a"][0].starts_with("Welcome"));
    assert_eq!(out.msgs["a"].len(), 1);
}

#[tokio::test]
async fn one_join() {
    let mut hanabi = Hanabi::default();
    let mut out = DummyMessageProxy::default();
    hanabi
        .on_dm_recv("join", SlackUserId("a".to_string()), &mut out)
        .await
        .unwrap();

    assert!(out.msgs.contains_key("a"));
    assert_eq!(out.msgs.len(), 1);
    assert!(
        out.msgs["a"][0].starts_with("Welcome"),
        "{}",
        out.msgs["a"][0]
    );
    assert_eq!(out.msgs["a"].len(), 1);
}

#[tokio::test]
async fn two_join() {
    let mut hanabi = Hanabi::default();
    let mut out = DummyMessageProxy::default();
    hanabi
        .on_dm_recv("join", SlackUserId("a".to_string()), &mut out)
        .await
        .unwrap();
    out.msgs.clear();
    hanabi
        .on_dm_recv("join", SlackUserId("b".to_string()), &mut out)
        .await
        .unwrap();

    assert!(out.msgs.contains_key("a"));
    assert!(out.msgs.contains_key("b"));
    assert_eq!(out.msgs.len(), 2);
    assert!(
        out.msgs["a"][0].starts_with("I have 1 other available player"),
        "{}",
        out.msgs["a"][0]
    );
    assert_eq!(out.msgs["a"].len(), 1);
    assert!(
        out.msgs["b"][0].starts_with("Welcome"),
        "{}",
        out.msgs["b"][0]
    );
    assert!(
        out.msgs["b"][1].starts_with("I have 1 other available player"),
        "{}",
        out.msgs["b"][1]
    );
    assert_eq!(out.msgs["b"].len(), 2);
}

#[tokio::test]
async fn start_alone() {
    let mut hanabi = Hanabi::default();
    let mut out = DummyMessageProxy::default();
    hanabi
        .on_dm_recv("join", SlackUserId("a".to_string()), &mut out)
        .await
        .unwrap();
    out.msgs.clear();
    hanabi
        .on_dm_recv("start", SlackUserId("a".to_string()), &mut out)
        .await
        .unwrap();

    assert!(out.msgs.contains_key("a"), "{out:?}");
    assert_eq!(out.msgs.len(), 1);
    assert_eq!(
        out.msgs["a"][0],
        "Unfortunately, there aren't enough players to start a game yet.",
    );
    assert_eq!(out.msgs["a"].len(), 1);
}

#[tokio::test]
async fn start() {
    let mut hanabi = Hanabi::default();
    let mut out = DummyMessageProxy::default();
    hanabi
        .on_dm_recv("join", SlackUserId("a".to_string()), &mut out)
        .await
        .unwrap();
    hanabi
        .on_dm_recv("join", SlackUserId("b".to_string()), &mut out)
        .await
        .unwrap();
    out.msgs.clear();
    hanabi
        .on_dm_recv("start", SlackUserId("a".to_string()), &mut out)
        .await
        .unwrap();

    assert!(out.msgs.contains_key("a"));
    assert!(out.msgs.contains_key("b"));
    assert_eq!(out.msgs.len(), 2);
    assert_eq!(
        out.msgs["a"][0],
        "You are now in a game with 1 other players: <@b>"
    );
    assert_ne!(out.msgs["a"].len(), 1);
    assert_eq!(
        out.msgs["b"][0],
        "You are now in a game with 1 other players: <@a>"
    );
    assert_eq!(
        out.msgs["b"][1],
        ":hourglass: It's <@a>'s turn; *8* :information_source: and 3 :bomb: remain."
    );
    assert_eq!(out.msgs["b"].len(), 2);
    // TODO: actually assert about game startup
}

#[derive(Debug, Default)]
struct DummyMessageProxy {
    msgs: HashMap<String, Vec<String>>,
}

impl MessageProxy for DummyMessageProxy {
    fn send(&mut self, user: &str, text: &str) {
        self.msgs
            .entry(user.to_string())
            .or_default()
            .push(text.to_owned());
    }
}
