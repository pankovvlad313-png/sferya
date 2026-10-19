//! Telegram history polling and local candidate extraction, independent of the login flow.
use std::future::Future;

use grammers_client::{Client, peer::Peer, tl};
use grammers_session::types::PeerRef;
use rusqlite::{Connection, OptionalExtension, params};

const PAGE_SIZE: i32 = 100;
const BATCH_SIZE: usize = 500;

fn person_name(first: Option<&str>, last: Option<&str>, username: Option<&str>, id: i64) -> String {
    let name = [
        first.unwrap_or_default().trim(),
        last.unwrap_or_default().trim(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    match (name.is_empty(), username.filter(|s| !s.is_empty())) {
        (false, Some(username)) => format!("{name} (@{username})"),
        (false, None) => name,
        (true, Some(username)) => format!("@{username}"),
        (true, None) => format!("Аккаунт {id}"),
    }
}

pub fn peer_name(peer: &Peer) -> String {
    match peer {
        Peer::User(user) if user.is_self() => "Избранное".into(),
        Peer::User(user) => person_name(
            user.first_name(),
            user.last_name(),
            user.username(),
            user.id().bot_api_dialog_id().unwrap_or_default(),
        ),
        _ => peer
            .name()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("Без названия")
            .to_owned(),
    }
}

pub fn peer_kind(peer: &Peer) -> &'static str {
    match peer {
        Peer::User(user) if user.is_self() => "Сохранённые сообщения",
        Peer::User(user) if user.is_bot() => "Бот",
        Peer::User(_) => "Личный чат",
        Peer::Group(_) => "Группа",
        Peer::Channel(_) => "Канал",
    }
}

#[derive(Clone, Debug)]
pub struct HistoryMessage {
    pub id: i32,
    pub outgoing: bool,
    pub sender: String,
    pub text: String,
    pub sent_at: String,
}

// grammers 0.10's reverse iterator sends min_id=offset_id. Telegram can return
// an empty page with that combination, even when new messages exist. Request
// the page immediately after an exclusive cursor and filter IDs locally, as
// Telethon does. Never use the newest page as the cursor when a backlog is >500.
fn history_request(peer: PeerRef, cursor: i32, limit: i32) -> tl::functions::messages::GetHistory {
    tl::functions::messages::GetHistory {
        peer: peer.into(),
        offset_id: cursor.saturating_add(1),
        offset_date: 0,
        add_offset: -limit,
        limit,
        max_id: 0,
        min_id: 0,
        hash: 0,
    }
}

fn history_page(
    response: tl::enums::messages::Messages,
    fallback_sender: &str,
) -> Result<Vec<HistoryMessage>, String> {
    use tl::enums::messages::Messages;
    let (messages, users, chats) = match response {
        Messages::Messages(page) => (page.messages, page.users, page.chats),
        Messages::Slice(page) => (page.messages, page.users, page.chats),
        Messages::ChannelMessages(page) => (page.messages, page.users, page.chats),
        Messages::NotModified(_) => return Err("Telegram вернул неожиданный ответ истории".into()),
    };
    let sender_name = |sender: &Option<tl::enums::Peer>| -> String {
        match sender {
            Some(tl::enums::Peer::User(peer)) => users.iter().find_map(|user| match user {
                tl::enums::User::User(user) if user.id == peer.user_id => Some(person_name(
                    user.first_name.as_deref(),
                    user.last_name.as_deref(),
                    user.username.as_deref(),
                    user.id,
                )),
                _ => None,
            }),
            Some(tl::enums::Peer::Channel(peer)) => chats.iter().find_map(|chat| match chat {
                tl::enums::Chat::Channel(chat) if chat.id == peer.channel_id => {
                    Some(chat.title.clone())
                }
                _ => None,
            }),
            _ => None,
        }
        .unwrap_or_else(|| fallback_sender.to_owned())
    };
    Ok(messages
        .into_iter()
        .map(|message| match message {
            tl::enums::Message::Message(message) => HistoryMessage {
                id: message.id,
                outgoing: message.out,
                sender: sender_name(&message.from_id),
                text: message.message,
                sent_at: chrono::DateTime::from_timestamp(i64::from(message.date), 0)
                    .map(|date| date.to_rfc3339())
                    .unwrap_or_default(),
            },
            // Deleted and service messages still advance the cursor.
            message => HistoryMessage {
                id: message.id(),
                outgoing: false,
                sender: String::new(),
                text: String::new(),
                sent_at: String::new(),
            },
        })
        .collect())
}

async fn read_history<F, Fut>(
    peer: PeerRef,
    last: i32,
    mut fetch: F,
) -> Result<Vec<HistoryMessage>, String>
where
    F: FnMut(tl::functions::messages::GetHistory) -> Fut,
    Fut: Future<Output = Result<Vec<HistoryMessage>, String>>,
{
    let mut result = Vec::new();
    let mut cursor = last;
    while result.len() < BATCH_SIZE && cursor < i32::MAX {
        let limit = PAGE_SIZE.min((BATCH_SIZE - result.len()) as i32);
        let mut page = fetch(history_request(peer, cursor, limit)).await?;
        page.retain(|message| message.id > cursor);
        page.sort_by_key(|message| message.id);
        page.dedup_by_key(|message| message.id);
        if page.is_empty() {
            break;
        }
        // A malformed response must not let us advance past unprocessed messages.
        page.truncate(limit as usize);
        cursor = page.last().unwrap().id;
        result.extend(page);
    }
    Ok(result)
}

pub async fn read_new_messages(
    client: &Client,
    peer: PeerRef,
    last: i32,
    name: &str,
) -> Result<Vec<HistoryMessage>, String> {
    read_history(peer, last, |request| async move {
        let response = client.invoke(&request).await.map_err(|e| e.to_string())?;
        history_page(response, name)
    })
    .await
}

pub fn enable_chat(conn: &Connection, id: &str, name: &str, latest: i32) -> Result<(), String> {
    // A repeated selected=true request is idempotent: it must not skip messages
    // which arrived since the first selection. Re-enabling starts at the present.
    conn.execute(
        "INSERT INTO telegram_chats(chat_id,name,enabled,last_message_id) VALUES(?1,?2,1,?3)
        ON CONFLICT(chat_id) DO UPDATE SET name=excluded.name,
        last_message_id=CASE WHEN telegram_chats.enabled=1 THEN telegram_chats.last_message_id
            ELSE MAX(telegram_chats.last_message_id,excluded.last_message_id) END,enabled=1",
        params![id, name, latest],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn save_batch(
    conn: &mut Connection,
    id: &str,
    name: &str,
    messages: &[HistoryMessage],
) -> Result<(), String> {
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let last = tx
        .query_row(
            "SELECT last_message_id FROM telegram_chats WHERE chat_id=?1 AND enabled=1",
            [id],
            |r| r.get::<_, i32>(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let Some(last) = last else {
        return Ok(());
    };
    let mut next = last;
    for message in messages.iter().filter(|m| m.id > last) {
        next = next.max(message.id);
        // Only incoming requests create candidates. Service and outgoing messages
        // still advance the high-water mark, preventing repeated scans.
        if message.outgoing {
            continue;
        }
        if let Some((confidence, bucket)) = classify_message(&message.text) {
            tx.execute("INSERT OR IGNORE INTO telegram_candidates(chat_id,message_id,chat_name,sender,body,sent_at,confidence,bucket)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![id, message.id, name, message.sender, message.text, message.sent_at, confidence, bucket])
                .map_err(|e| e.to_string())?;
        }
    }
    tx.execute("UPDATE telegram_chats SET name=?1,last_message_id=MAX(last_message_id,?2) WHERE chat_id=?3",
        params![name, next, id]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}

/// Conservative local rules; ambiguous requests are reviewed in "Возможно, задачи".
fn classify_message(text: &str) -> Option<(f64, &'static str)> {
    let lower = text.trim().to_lowercase().replace('ё', "е");
    let words = lower
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        return None;
    }
    let direct = [
        "сделай",
        "сделайте",
        "подготовь",
        "подготовьте",
        "отправь",
        "отправьте",
        "проверь",
        "проверьте",
        "пришли",
        "пришлите",
        "напомни",
        "напомните",
        "купи",
        "купите",
        "позвони",
        "позвоните",
        "посмотри",
        "посмотрите",
        "помоги",
        "помогите",
        "заполни",
        "заполните",
        "согласуй",
        "согласуйте",
        "оплати",
        "оплатите",
        "добавь",
        "добавьте",
        "исправь",
        "исправьте",
        "забери",
        "заберите",
        "передай",
        "передайте",
        "закажи",
        "закажите",
        "ответь",
        "ответьте",
        "отправить",
        "сделать",
        "подготовить",
        "проверить",
        "купить",
    ];
    // Match words, not substrings ("позвонил" and "задача решена" are not requests).
    if words.iter().any(|word| direct.contains(word))
        || lower.starts_with("задача:")
        || lower.starts_with("todo:")
    {
        return Some((0.85, "high"));
    }
    let uncertain = [
        "нужно",
        "надо",
        "нужен",
        "нужна",
        "нужны",
        "нужное",
        "необходимо",
        "требуется",
        "пожалуйста",
        "можешь",
        "можете",
        "сможешь",
        "сможете",
        "дедлайн",
        "пришлешь",
        "отправишь",
        "сделаешь",
        "подготовишь",
        "проверишь",
        "посмотришь",
        "позвонишь",
        "скинешь",
        "скинь",
        "скиньте",
        "скинуть",
        "созвониться",
        "прислать",
        "позвонить",
        "заполнить",
        "оплатить",
        "согласовать",
    ];
    if words.iter().any(|word| uncertain.contains(word))
        || words.windows(2).any(|pair| {
            matches!(
                pair,
                ["не", "забудь" | "забудьте"]
                    | ["на", "завтра" | "сегодня"]
                    | ["до", "пятницы" | "понедельника"]
            )
        })
    {
        Some((0.5, "maybe"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grammers_session::types::{PeerAuth, PeerId};

    fn peer() -> PeerRef {
        PeerRef {
            id: PeerId::user_unchecked(42),
            auth: PeerAuth::from_hash(123),
        }
    }
    fn message(id: i32, text: &str) -> HistoryMessage {
        HistoryMessage {
            id,
            outgoing: false,
            sender: "Анна Иванова".into(),
            text: text.into(),
            sent_at: "2026-10-19T00:00:00Z".into(),
        }
    }
    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("schema.sql")).unwrap();
        enable_chat(&conn, "42", "Анна Иванова", 10).unwrap();
        conn
    }
    fn cursor(conn: &Connection) -> i32 {
        conn.query_row(
            "SELECT last_message_id FROM telegram_chats WHERE chat_id='42'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }
    fn count(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM telegram_candidates", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn names_distinguish_people_and_search_by_username() {
        assert_eq!(
            person_name(Some("Анна"), Some("Иванова"), Some("anna"), 42),
            "Анна Иванова (@anna)"
        );
        assert_eq!(
            person_name(Some("Анна"), Some("Петрова"), None, 43),
            "Анна Петрова"
        );
        assert_eq!(person_name(None, None, Some("anna"), 42), "@anna");
        assert_eq!(person_name(None, None, None, 42), "Аккаунт 42");
    }

    #[test]
    fn usual_requests_are_candidates_but_conversation_is_not() {
        for text in [
            "Подготовь отчёт",
            "Купи хлеб",
            "Посмотри документ",
            "Проверьте смету",
            "Задача: новый экран",
        ] {
            assert_eq!(classify_message(text), Some((0.85, "high")), "{text}");
        }
        for text in [
            "Скинешь отчёт?",
            "Пришлёшь договор?",
            "Нужен отчет",
            "Можешь посмотреть завтра?",
            "Не забудь про встречу",
        ] {
            assert!(classify_message(text).is_some(), "{text}");
        }
        for text in [
            "Доброе утро",
            "Спасибо!",
            "Я позвонил вчера",
            "Отправлено",
            "Задача решена",
            "",
        ] {
            assert_eq!(classify_message(text), None, "{text}");
        }
    }

    // Fake Telegram implements descending GetHistory pagination and its empty-page
    // behavior for min_id>0. This test exercises the actual request builder and loop.
    fn server(
        request: tl::functions::messages::GetHistory,
        messages: &[HistoryMessage],
    ) -> Vec<HistoryMessage> {
        if request.min_id != 0 {
            return vec![];
        }
        let descending = messages.iter().rev().cloned().collect::<Vec<_>>();
        let offset = descending
            .iter()
            .position(|m| m.id < request.offset_id)
            .unwrap_or(descending.len());
        let start = (offset as i32 + request.add_offset).max(0) as usize;
        descending
            .into_iter()
            .skip(start)
            .take(request.limit as usize)
            .collect()
    }

    #[tokio::test]
    async fn receives_single_message_after_selection_and_does_not_import_history() {
        let history = (1..=11)
            .map(|id| message(id, "Проверь отчет"))
            .collect::<Vec<_>>();
        let read = read_history(peer(), 10, |request| {
            std::future::ready(Ok(server(request, &history)))
        })
        .await
        .unwrap();
        assert_eq!(read.iter().map(|m| m.id).collect::<Vec<_>>(), vec![11]);
        let mut conn = database();
        save_batch(&mut conn, "42", "Анна Иванова", &read).unwrap();
        assert_eq!(count(&conn), 1);
        assert_eq!(cursor(&conn), 11);
    }

    #[tokio::test]
    async fn backlog_over_batch_limit_is_not_lost_and_sparse_ids_work() {
        let history = (1..=635)
            .map(|id| message(id * 3, "Пришли отчет"))
            .collect::<Vec<_>>();
        let first = read_history(peer(), 30, |request| {
            std::future::ready(Ok(server(request, &history)))
        })
        .await
        .unwrap();
        assert_eq!(first.len(), 500);
        assert_eq!(first.first().unwrap().id, 33);
        assert_eq!(first.last().unwrap().id, 1530);
        let second = read_history(peer(), 1530, |request| {
            std::future::ready(Ok(server(request, &history)))
        })
        .await
        .unwrap();
        assert_eq!(second.len(), 125);
        assert_eq!(second.first().unwrap().id, 1533);
        assert_eq!(second.last().unwrap().id, 1905);
    }

    #[tokio::test]
    async fn empty_chat_starts_with_first_new_message() {
        let history = vec![message(1, "Купи хлеб")];
        let read = read_history(peer(), 0, |request| {
            std::future::ready(Ok(server(request, &history)))
        })
        .await
        .unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].id, 1);
    }

    #[tokio::test]
    async fn network_failure_does_not_advance_database_cursor() {
        let conn = database();
        let mut requests = 0;
        let read = read_history(peer(), 10, |_| {
            requests += 1;
            std::future::ready(if requests == 1 {
                Ok(vec![message(11, "Проверь отчет")])
            } else {
                Err("offline".into())
            })
        })
        .await;
        assert_eq!(read.unwrap_err(), "offline");
        assert_eq!(cursor(&conn), 10);
        assert_eq!(count(&conn), 0);
    }

    #[test]
    fn duplicate_selection_does_not_skip_unread_messages() {
        let conn = database();
        enable_chat(&conn, "42", "Анна Иванова (@anna)", 20).unwrap();
        assert_eq!(cursor(&conn), 10);
        conn.execute("UPDATE telegram_chats SET enabled=0", [])
            .unwrap();
        enable_chat(&conn, "42", "Анна Иванова", 25).unwrap();
        assert_eq!(cursor(&conn), 25);
    }

    #[test]
    fn only_selected_incoming_requests_are_saved_without_duplicates() {
        let mut conn = database();
        let mut outgoing = message(12, "Пришли отчет");
        outgoing.outgoing = true;
        let messages = vec![
            message(10, "Старое"),
            message(11, "Купи хлеб"),
            outgoing,
            message(13, "Спасибо"),
            message(14, ""),
        ];
        save_batch(&mut conn, "42", "Анна Иванова", &messages).unwrap();
        save_batch(&mut conn, "42", "Анна Иванова", &messages).unwrap();
        save_batch(
            &mut conn,
            "unselected",
            "Чужой чат",
            &[message(15, "Купи хлеб")],
        )
        .unwrap();
        assert_eq!(count(&conn), 1);
        assert_eq!(cursor(&conn), 14);
        conn.execute("UPDATE telegram_chats SET enabled=0", [])
            .unwrap();
        save_batch(&mut conn, "42", "Анна Иванова", &[message(15, "Купи хлеб")]).unwrap();
        assert_eq!(count(&conn), 1);
        assert_eq!(cursor(&conn), 14);
    }

    #[test]
    fn candidate_failure_rolls_back_cursor_and_partial_inserts() {
        let mut conn = database();
        conn.execute_batch(
            "CREATE TRIGGER reject_candidate BEFORE INSERT ON telegram_candidates
            WHEN NEW.message_id=12 BEGIN SELECT RAISE(ABORT,'fixture failure'); END;",
        )
        .unwrap();
        assert!(
            save_batch(
                &mut conn,
                "42",
                "Анна Иванова",
                &[message(11, "Купи хлеб"), message(12, "Пришли отчет")]
            )
            .is_err()
        );
        assert_eq!(count(&conn), 0);
        assert_eq!(cursor(&conn), 10);
    }
}
