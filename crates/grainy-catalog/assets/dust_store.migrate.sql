CREATE TABLE IF NOT EXISTS dust_sessions (
            id TEXT PRIMARY KEY,version INTEGER NOT NULL,target_key TEXT NOT NULL,
            scope TEXT NOT NULL,snapshot TEXT NOT NULL,options TEXT NOT NULL,
            created TEXT NOT NULL,updated TEXT NOT NULL,current_index INTEGER NOT NULL,
            revision INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS dust_entries (
            session_id TEXT NOT NULL,ordinal INTEGER NOT NULL,photo_id INTEGER NOT NULL,
            path TEXT NOT NULL,state TEXT NOT NULL,payload BLOB NOT NULL,
            PRIMARY KEY(session_id,ordinal));
