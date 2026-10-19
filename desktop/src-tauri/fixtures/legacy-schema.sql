PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
      CREATE TABLE IF NOT EXISTS areas (id INTEGER PRIMARY KEY, name TEXT NOT NULL CHECK(length(trim(name))>0), icon TEXT NOT NULL DEFAULT 'circle', color TEXT NOT NULL DEFAULT '#61856c', created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')));
      CREATE TABLE IF NOT EXISTS projects (id INTEGER PRIMARY KEY, area_id INTEGER NOT NULL REFERENCES areas(id), name TEXT NOT NULL CHECK(length(trim(name))>0), created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')), UNIQUE(id,area_id));
      CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY, area_id INTEGER REFERENCES areas(id), project_id INTEGER, title TEXT NOT NULL CHECK(length(trim(title))>0), description TEXT NOT NULL DEFAULT '', url TEXT NOT NULL DEFAULT '', due_date TEXT, completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)), created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')), updated_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')), CHECK(project_id IS NULL OR area_id IS NOT NULL), FOREIGN KEY(project_id,area_id) REFERENCES projects(id,area_id));
      CREATE TABLE IF NOT EXISTS app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
      INSERT OR IGNORE INTO app_settings VALUES ('schema_version','1');
      CREATE INDEX IF NOT EXISTS tasks_due ON tasks(completed,due_date);
      CREATE INDEX IF NOT EXISTS tasks_area ON tasks(area_id,project_id);
BEGIN;
      CREATE TABLE IF NOT EXISTS subtasks (task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, position INTEGER NOT NULL, title TEXT NOT NULL CHECK(length(trim(title))>0), completed INTEGER NOT NULL CHECK(completed IN (0,1)), PRIMARY KEY(task_id,position));
      UPDATE app_settings SET value='2' WHERE key='schema_version';
      CREATE TABLE IF NOT EXISTS task_order (task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE, position INTEGER NOT NULL);
      UPDATE app_settings SET value='3' WHERE key='schema_version';
      CREATE TABLE IF NOT EXISTS file_assets (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL, size INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS task_materials (task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, position INTEGER NOT NULL, content TEXT NOT NULL, asset_id TEXT REFERENCES file_assets(id), PRIMARY KEY(task_id,position));
      CREATE TABLE IF NOT EXISTS task_rich_descriptions(task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,content TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS task_trash(task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,deleted_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')));
      CREATE TABLE IF NOT EXISTS project_materials(project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,position INTEGER NOT NULL,content TEXT NOT NULL,asset_id TEXT REFERENCES file_assets(id),PRIMARY KEY(project_id,position));
      CREATE TABLE IF NOT EXISTS subtask_keys(task_id INTEGER NOT NULL,subtask_position INTEGER NOT NULL,id TEXT NOT NULL,PRIMARY KEY(task_id,subtask_position),FOREIGN KEY(task_id,subtask_position) REFERENCES subtasks(task_id,position) ON DELETE CASCADE);
      CREATE TABLE IF NOT EXISTS subtask_materials(task_id INTEGER NOT NULL,subtask_position INTEGER NOT NULL,position INTEGER NOT NULL,content TEXT NOT NULL,asset_id TEXT REFERENCES file_assets(id),PRIMARY KEY(task_id,subtask_position,position),FOREIGN KEY(task_id,subtask_position) REFERENCES subtasks(task_id,position) ON DELETE CASCADE);
      CREATE TABLE IF NOT EXISTS area_materials(area_id INTEGER NOT NULL REFERENCES areas(id) ON DELETE CASCADE,position INTEGER NOT NULL,content TEXT NOT NULL,asset_id TEXT REFERENCES file_assets(id),PRIMARY KEY(area_id,position));
      CREATE TABLE IF NOT EXISTS task_kanban_stage(task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,stage TEXT NOT NULL CHECK(stage IN ('progress','review')));
      UPDATE app_settings SET value='9' WHERE key='schema_version';
      COMMIT;
CREATE TABLE IF NOT EXISTS telegram_chats (
      chat_id TEXT PRIMARY KEY, name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
      last_message_id INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE IF NOT EXISTS telegram_candidates (
      chat_id TEXT NOT NULL, message_id INTEGER NOT NULL, chat_name TEXT NOT NULL,
      sender TEXT NOT NULL, body TEXT NOT NULL, sent_at TEXT NOT NULL,
      status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','accepted','dismissed')),
      task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL,
      PRIMARY KEY(chat_id,message_id));
      CREATE INDEX IF NOT EXISTS telegram_candidates_status ON telegram_candidates(status,sent_at);
CREATE TABLE IF NOT EXISTS telegram_task_sources (
      task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
      chat_id TEXT NOT NULL, message_id INTEGER NOT NULL);
      INSERT OR IGNORE INTO telegram_task_sources(task_id,chat_id,message_id)
      SELECT task_id,chat_id,message_id FROM telegram_candidates WHERE status='accepted' AND task_id IS NOT NULL;