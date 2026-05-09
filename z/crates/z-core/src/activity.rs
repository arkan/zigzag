use std::collections::HashMap;

use crate::domain::Session;
use crate::error::Result;

pub type SessionActivity = HashMap<String, u64>;

/// Storage Interface for persisted Session activity.
pub trait ActivityStore {
    fn record_attach(&self, session: &str) -> Result<()>;
    fn load_activity(&self) -> SessionActivity;
    fn remove_entry(&self, session: &str) -> Result<()>;
}

/// Sort `sessions` in-place by their last-attach timestamp, most recent first.
/// Sessions without a recorded timestamp are placed at the end in their
/// original relative order (stable sort).
pub fn sort_sessions_by_recent_attach(sessions: &mut [Session], activity: &SessionActivity) {
    sort_by_recent_attach(sessions, activity, |s| &s.name);
}

/// Stable-sort `items` in-place by last-attach timestamp, most recent first.
/// Items whose Session name has no recorded timestamp are placed at the end
/// in their original relative order. `name_of` extracts the Session name
/// (format `project:branch`) from each item.
pub fn sort_by_recent_attach<T, F>(items: &mut [T], activity: &SessionActivity, name_of: F)
where
    F: Fn(&T) -> &str,
{
    items.sort_by(|a, b| {
        let ta = activity.get(name_of(a)).copied().unwrap_or(0);
        let tb = activity.get(name_of(b)).copied().unwrap_or(0);
        tb.cmp(&ta)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(name: &str) -> Session {
        let (project, branch) = name.split_once(':').unwrap();
        Session::new(project, branch)
    }

    #[test]
    fn sort_puts_most_recent_attach_first() {
        let mut sessions = vec![sess("app:old"), sess("app:new"), sess("app:mid")];
        let activity: SessionActivity = [
            ("app:old".to_string(), 100),
            ("app:new".to_string(), 300),
            ("app:mid".to_string(), 200),
        ]
        .into_iter()
        .collect();
        sort_sessions_by_recent_attach(&mut sessions, &activity);
        assert_eq!(
            sessions.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["app:new", "app:mid", "app:old"]
        );
    }

    #[test]
    fn sort_by_recent_attach_works_on_string_tuples() {
        let mut items: Vec<(String, u32)> = vec![
            ("app:a".to_string(), 1),
            ("app:b".to_string(), 2),
            ("app:c".to_string(), 3),
        ];
        let activity: SessionActivity = [
            ("app:a".to_string(), 100),
            ("app:b".to_string(), 500),
            ("app:c".to_string(), 250),
        ]
        .into_iter()
        .collect();
        sort_by_recent_attach(&mut items, &activity, |it| it.0.as_str());
        assert_eq!(
            items.iter().map(|it| it.0.as_str()).collect::<Vec<_>>(),
            vec!["app:b", "app:c", "app:a"]
        );
    }

    #[test]
    fn sort_places_sessions_without_activity_at_end() {
        let mut sessions = vec![sess("app:unknown"), sess("app:recent"), sess("app:ancient")];
        let activity: SessionActivity = [
            ("app:recent".to_string(), 500),
            ("app:ancient".to_string(), 10),
        ]
        .into_iter()
        .collect();
        sort_sessions_by_recent_attach(&mut sessions, &activity);
        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["app:recent", "app:ancient", "app:unknown"]);
    }
}
