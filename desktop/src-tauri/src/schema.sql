-- Exact sqlite_schema SQL observed from native isolated app backup.
-- Backup SHA256 0d422037ccf78700b242b6184a3ac8f8942e41b9fc640cca4d6112106de5fd66
CREATE TABLE allocated_ids(kind TEXT NOT NULL,id INTEGER NOT NULL,PRIMARY KEY(kind,id));

CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE area_materials(area_id INTEGER NOT NULL REFERENCES areas(id) ON DELETE CASCADE,position INTEGER NOT NULL,content TEXT NOT NULL,asset_id TEXT REFERENCES file_assets(id),PRIMARY KEY(area_id,position));

CREATE TABLE areas (id INTEGER PRIMARY KEY, name TEXT NOT NULL CHECK(length(trim(name))>0), icon TEXT NOT NULL DEFAULT 'circle', color TEXT NOT NULL DEFAULT '#61856c', created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')));

CREATE TABLE file_assets (id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL, size INTEGER NOT NULL);

CREATE TABLE project_materials(project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,position INTEGER NOT NULL,content TEXT NOT NULL,asset_id TEXT REFERENCES file_assets(id),PRIMARY KEY(project_id,position));

CREATE TABLE projects (id INTEGER PRIMARY KEY, area_id INTEGER NOT NULL REFERENCES areas(id), name TEXT NOT NULL CHECK(length(trim(name))>0), created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')), UNIQUE(id,area_id));

CREATE TABLE subtask_keys(task_id INTEGER NOT NULL,subtask_position INTEGER NOT NULL,id TEXT NOT NULL,PRIMARY KEY(task_id,subtask_position),FOREIGN KEY(task_id,subtask_position) REFERENCES subtasks(task_id,position) ON DELETE CASCADE);

CREATE TABLE subtask_materials(task_id INTEGER NOT NULL,subtask_position INTEGER NOT NULL,position INTEGER NOT NULL,content TEXT NOT NULL,asset_id TEXT REFERENCES file_assets(id),PRIMARY KEY(task_id,subtask_position,position),FOREIGN KEY(task_id,subtask_position) REFERENCES subtasks(task_id,position) ON DELETE CASCADE);

CREATE TABLE subtasks (task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, position INTEGER NOT NULL, title TEXT NOT NULL CHECK(length(trim(title))>0), completed INTEGER NOT NULL CHECK(completed IN (0,1)), PRIMARY KEY(task_id,position));

CREATE TABLE task_kanban_stage(task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,stage TEXT NOT NULL CHECK(stage IN ('progress','review')));

CREATE TABLE task_materials (task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, position INTEGER NOT NULL, content TEXT NOT NULL, asset_id TEXT REFERENCES file_assets(id), PRIMARY KEY(task_id,position));

CREATE TABLE task_order (task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE, position INTEGER NOT NULL);

CREATE TABLE task_rich_descriptions(task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,content TEXT NOT NULL);

CREATE TABLE task_trash(task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,deleted_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')));

CREATE TABLE tasks (id INTEGER PRIMARY KEY, area_id INTEGER REFERENCES areas(id), project_id INTEGER, title TEXT NOT NULL CHECK(length(trim(title))>0), description TEXT NOT NULL DEFAULT '', url TEXT NOT NULL DEFAULT '', due_date TEXT, completed INTEGER NOT NULL DEFAULT 0 CHECK(completed IN (0,1)), created_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')), updated_at TEXT NOT NULL DEFAULT(strftime('%Y-%m-%dT%H:%M:%fZ','now')), completed_at TEXT, CHECK(project_id IS NULL OR area_id IS NOT NULL), FOREIGN KEY(project_id,area_id) REFERENCES projects(id,area_id));

CREATE TABLE telegram_candidates (
      chat_id TEXT NOT NULL, message_id INTEGER NOT NULL, chat_name TEXT NOT NULL,
      sender TEXT NOT NULL, body TEXT NOT NULL, sent_at TEXT NOT NULL,
      status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','accepted','dismissed')),
      task_id INTEGER REFERENCES tasks(id) ON DELETE SET NULL, confidence REAL NOT NULL DEFAULT 1.0, bucket TEXT NOT NULL DEFAULT 'high',
      PRIMARY KEY(chat_id,message_id));

CREATE TABLE telegram_chats (
      chat_id TEXT PRIMARY KEY, name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
      last_message_id INTEGER NOT NULL DEFAULT 0);

CREATE TABLE telegram_task_sources (
      task_id INTEGER PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
      chat_id TEXT NOT NULL, message_id INTEGER NOT NULL);

CREATE INDEX tasks_area ON tasks(area_id,project_id);

CREATE INDEX tasks_due ON tasks(completed,due_date);

CREATE INDEX telegram_candidates_chat_status ON telegram_candidates(chat_id,status);

CREATE INDEX telegram_candidates_status ON telegram_candidates(status,sent_at);

CREATE TRIGGER areas_id_high_water AFTER INSERT ON areas BEGIN INSERT INTO app_settings(key,value) VALUES('id_high_water_areas',CAST(NEW.id AS TEXT)) ON CONFLICT(key) DO UPDATE SET value=CAST(MAX(CAST(app_settings.value AS INTEGER),NEW.id) AS TEXT); END;

CREATE TRIGGER areas_reserve_id AFTER INSERT ON areas BEGIN INSERT OR IGNORE INTO allocated_ids(kind,id) VALUES('areas',NEW.id); END;

CREATE TRIGGER projects_id_high_water AFTER INSERT ON projects BEGIN INSERT INTO app_settings(key,value) VALUES('id_high_water_projects',CAST(NEW.id AS TEXT)) ON CONFLICT(key) DO UPDATE SET value=CAST(MAX(CAST(app_settings.value AS INTEGER),NEW.id) AS TEXT); END;

CREATE TRIGGER projects_reserve_id AFTER INSERT ON projects BEGIN INSERT OR IGNORE INTO allocated_ids(kind,id) VALUES('projects',NEW.id); END;

CREATE TRIGGER task_completion_insert AFTER INSERT ON tasks BEGIN UPDATE tasks SET completed_at=CASE WHEN NEW.completed=1 THEN COALESCE(NEW.completed_at,NEW.updated_at) ELSE NULL END WHERE id=NEW.id; END;

CREATE TRIGGER task_completion_update AFTER UPDATE OF completed ON tasks BEGIN UPDATE tasks SET completed_at=CASE WHEN NEW.completed=0 THEN NULL WHEN OLD.completed=0 THEN COALESCE(NEW.completed_at,strftime('%Y-%m-%dT%H:%M:%fZ','now')) ELSE COALESCE(NEW.completed_at,OLD.completed_at,OLD.updated_at) END WHERE id=NEW.id; END;

CREATE TRIGGER tasks_id_high_water AFTER INSERT ON tasks BEGIN INSERT INTO app_settings(key,value) VALUES('id_high_water_tasks',CAST(NEW.id AS TEXT)) ON CONFLICT(key) DO UPDATE SET value=CAST(MAX(CAST(app_settings.value AS INTEGER),NEW.id) AS TEXT); END;

CREATE TRIGGER tasks_reserve_id AFTER INSERT ON tasks BEGIN INSERT OR IGNORE INTO allocated_ids(kind,id) VALUES('tasks',NEW.id); END;
