# Synthetic compatibility fixtures

original-probe.sqlite is a synthetic database retained from the prior reconstruction at Documents/Codex/2026-10-12/x20-2/outputs/sfery-source/desktop/src-tauri/fixtures. It contains only the explicitly identified recovery probe (task8418832499662761, subtask probe-sub, 20-byte probe.txt). It is not the user current database. SHA256: 1aa5c09d2addff3b4d46cf0558384d2197d36f0545b3eebd0c9cd25ba11c1897.

legacy-schema.sql concatenates exact shipped app SQL byte ranges001,002,005,006 from contracts/schema-evidence.json. Tests create a separate temporary legacy database and verify additive completed_at/confidence/bucket migration preserves synthetic records.
