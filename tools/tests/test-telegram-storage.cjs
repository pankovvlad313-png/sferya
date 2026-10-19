// Runs the production Telegram SQL against disposable SQLite databases without
// executing a locally compiled EXE or opening the user's Telegram session.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { DatabaseSync } = require('node:sqlite');

const packagedRust = path.resolve(__dirname, '../../source/desktop/src-tauri/src');
const rust = fs.existsSync(packagedRust) ? packagedRust : path.resolve(__dirname, '../../desktop/src-tauri/src');
const source = fs.readFileSync(path.join(rust, 'telegram.rs'), 'utf8');
function sql(prefix) {
  const matches = [...source.matchAll(/"((?:[^"\\]|\\.)*)"/g)]
    .map(match => match[1]).filter(text => text.startsWith(prefix));
  assert.equal(matches.length, 1, `Expected exactly one production SQL statement: ${prefix}`);
  return matches[0];
}
const enableSql = sql('INSERT INTO telegram_chats');
const candidateSql = sql('INSERT OR IGNORE INTO telegram_candidates');
const advanceSql = sql('UPDATE telegram_chats SET name=');

function fixture(run) {
  const db = new DatabaseSync(':memory:');
  try {
    db.exec('PRAGMA foreign_keys=ON');
    db.exec(fs.readFileSync(path.join(rust, 'schema.sql'), 'utf8'));
    db.prepare(enableSql).run({ 1: '42', 2: 'Анна Иванова', 3: 10 });
    run(db);
  } finally { db.close(); }
}
const chat = db => db.prepare('SELECT * FROM telegram_chats WHERE chat_id=?').get('42');
const candidate = (db, id) => db.prepare(candidateSql).run({
  1: '42', 2: id, 3: 'Анна Иванова', 4: 'Анна Иванова (@anna)',
  5: 'Купи хлеб', 6: '2026-10-19T00:00:00Z', 7: 0.85, 8: 'high',
});

test('repeated selection updates the full name without skipping unread messages', () => fixture(db => {
  db.prepare(enableSql).run({ 1: '42', 2: 'Анна Иванова (@anna)', 3: 20 });
  assert.equal(chat(db).name, 'Анна Иванова (@anna)');
  assert.equal(chat(db).last_message_id, 10);
  assert.equal(chat(db).enabled, 1);
}));

test('re-enabling starts at the present and never rewinds the existing cursor', () => fixture(db => {
  db.exec('UPDATE telegram_chats SET enabled=0');
  db.prepare(enableSql).run({ 1: '42', 2: 'Анна Иванова', 3: 20 });
  assert.equal(chat(db).last_message_id, 20);
  db.exec('UPDATE telegram_chats SET enabled=0');
  db.prepare(enableSql).run({ 1: '42', 2: 'Анна Иванова', 3: 15 });
  assert.equal(chat(db).last_message_id, 20);
}));

test('replaying a message preserves a dismissed candidate and creates no duplicate', () => fixture(db => {
  candidate(db, 11);
  db.exec("UPDATE telegram_candidates SET status='dismissed'");
  candidate(db, 11);
  const saved = db.prepare('SELECT * FROM telegram_candidates').all();
  assert.equal(saved.length, 1);
  assert.equal(saved[0].status, 'dismissed');
  assert.equal(saved[0].sender, 'Анна Иванова (@anna)');
}));

test('a failed batch rolls back both candidate insertion and cursor advancement', () => fixture(db => {
  db.exec("CREATE TRIGGER reject_candidate BEFORE INSERT ON telegram_candidates WHEN NEW.message_id=12 BEGIN SELECT RAISE(ABORT,'fixture failure'); END;");
  db.exec('BEGIN');
  assert.throws(() => {
    candidate(db, 11);
    db.prepare(advanceSql).run({ 1: 'Анна Иванова', 2: 12, 3: '42' });
    candidate(db, 12);
  }, /fixture failure/);
  db.exec('ROLLBACK');
  assert.equal(db.prepare('SELECT count(*) AS n FROM telegram_candidates').get().n, 0);
  assert.equal(chat(db).last_message_id, 10);
}));
