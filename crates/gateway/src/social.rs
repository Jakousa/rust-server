//! Gateway subscription manager for client social and event pub/sub fan-out.
//!
//! Maintains client topic subscriptions across the gateway connection lifecycle
//! with strict DashMap lock isolation (never holding nested locks).

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use std::collections::HashSet;

/// Lock-isolated subscription manager tracking topic subscribers and session topics.
#[derive(Debug, Default)]
pub struct GatewaySubscriptionManager {
    topic_subscribers: DashMap<String, HashSet<u64>>,
    session_topics: DashMap<u64, HashSet<String>>,
}

impl GatewaySubscriptionManager {
    /// Creates a new, empty subscription manager.
    pub fn new() -> Self {
        Self {
            topic_subscribers: DashMap::new(),
            session_topics: DashMap::new(),
        }
    }

    /// Subscribes a session to a named topic.
    ///
    /// Returns `true` if this was a new subscription for the session.
    pub fn subscribe(&self, session_id: u64, topic: String) -> bool {
        let mut was_newly_subscribed = false;

        // Step 1: Update session_topics in an isolated scope
        {
            let entry = self.session_topics.entry(session_id);
            match entry {
                Entry::Occupied(mut e) => {
                    let topics = e.get_mut();
                    if !topics.contains(&topic) {
                        topics.insert(topic.clone());
                        was_newly_subscribed = true;
                    }
                }
                Entry::Vacant(e) => {
                    e.insert(HashSet::from([topic.clone()]));
                    was_newly_subscribed = true;
                }
            }
        } // Drop lock on session_topics immediately before accessing topic_subscribers

        // Step 2: Update topic_subscribers in an isolated scope
        {
            let entry = self.topic_subscribers.entry(topic);
            match entry {
                Entry::Occupied(mut e) => {
                    e.get_mut().insert(session_id);
                }
                Entry::Vacant(e) => {
                    e.insert(HashSet::from([session_id]));
                }
            }
        } // Drop lock on topic_subscribers immediately

        was_newly_subscribed
    }

    /// Unsubscribes a session from a named topic.
    ///
    /// Returns `true` if the session was actively subscribed to the topic.
    pub fn unsubscribe(&self, session_id: u64, topic: &str) -> bool {
        let mut was_subscribed = false;

        // Step 1: Remove from session_topics in an isolated scope
        {
            if let Some(mut topics) = self.session_topics.get_mut(&session_id) {
                if topics.remove(topic) {
                    was_subscribed = true;
                }
            }
        } // Drop lock on session_topics

        // Step 2: Remove from topic_subscribers in an isolated scope
        {
            let mut remove_topic = false;
            if let Some(mut subscribers) = self.topic_subscribers.get_mut(topic) {
                subscribers.remove(&session_id);
                if subscribers.is_empty() {
                    remove_topic = true;
                }
            }
            if remove_topic {
                self.topic_subscribers.remove(topic);
            }
        } // Drop lock on topic_subscribers

        was_subscribed
    }

    /// Returns a snapshot list of session IDs subscribed to `topic`.
    pub fn get_subscribers(&self, topic: &str) -> Vec<u64> {
        self.topic_subscribers
            .get(topic)
            .map(|subscribers| subscribers.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Returns a snapshot list of topics the session is currently subscribed to.
    pub fn get_session_topics(&self, session_id: u64) -> Vec<String> {
        self.session_topics
            .get(&session_id)
            .map(|topics| topics.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Cleanly deregisters a session and all its topic subscriptions upon disconnect.
    ///
    /// Operates in strict two-phase lock isolation:
    /// 1. Extracts and removes the session's topic set from `session_topics`.
    /// 2. Iterates over the detached topic set and removes `session_id` from each `topic_subscribers` entry.
    pub fn remove_session(&self, session_id: u64) -> Vec<String> {
        let topics_set = self
            .session_topics
            .remove(&session_id)
            .map(|(_, topics)| topics)
            .unwrap_or_default();

        let mut removed_topics = Vec::with_capacity(topics_set.len());
        for topic in topics_set {
            let mut remove_topic = false;
            if let Some(mut subscribers) = self.topic_subscribers.get_mut(&topic) {
                subscribers.remove(&session_id);
                if subscribers.is_empty() {
                    remove_topic = true;
                }
            }
            if remove_topic {
                self.topic_subscribers.remove(&topic);
            }
            removed_topics.push(topic);
        }

        removed_topics
    }

    /// Returns the total number of distinct active topics.
    pub fn topic_count(&self) -> usize {
        self.topic_subscribers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscribe_unsubscribe() {
        let manager = GatewaySubscriptionManager::new();
        assert!(manager.subscribe(1, "topic1".to_string()));
        assert!(!manager.subscribe(1, "topic1".to_string()));
        assert!(manager.unsubscribe(1, "topic1"));
        assert!(!manager.unsubscribe(1, "topic1"));
    }

    #[test]
    fn test_get_subscribers() {
        let manager = GatewaySubscriptionManager::new();
        manager.subscribe(1, "topic1".to_string());
        manager.subscribe(2, "topic1".to_string());
        let mut subs = manager.get_subscribers("topic1");
        subs.sort_unstable();
        assert_eq!(subs, vec![1, 2]);
    }

    #[test]
    fn test_get_session_topics() {
        let manager = GatewaySubscriptionManager::new();
        manager.subscribe(1, "topic1".to_string());
        manager.subscribe(1, "topic2".to_string());
        let mut topics = manager.get_session_topics(1);
        topics.sort();
        assert_eq!(topics, vec!["topic1".to_string(), "topic2".to_string()]);
    }

    #[test]
    fn test_remove_session() {
        let manager = GatewaySubscriptionManager::new();
        manager.subscribe(1, "topic1".to_string());
        manager.subscribe(1, "topic2".to_string());
        let mut removed = manager.remove_session(1);
        removed.sort();
        assert_eq!(removed, vec!["topic1".to_string(), "topic2".to_string()]);
        assert!(manager.topic_subscribers.get("topic1").is_none());
        assert!(manager.topic_subscribers.get("topic2").is_none());
        assert_eq!(manager.topic_count(), 0);
    }

    #[test]
    fn test_concurrency_safety() {
        let manager = std::sync::Arc::new(GatewaySubscriptionManager::new());
        let mut handles = Vec::new();
        for i in 0..10 {
            let mgr = manager.clone();
            handles.push(std::thread::spawn(move || {
                for j in 0..100 {
                    let topic = format!("topic{}", j % 5);
                    mgr.subscribe(i, topic.clone());
                    mgr.unsubscribe(i, &topic);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }
    }
}
