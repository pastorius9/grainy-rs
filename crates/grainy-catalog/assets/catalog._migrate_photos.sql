CREATE TABLE photos_v2 (
                id INTEGER PRIMARY KEY,path TEXT NOT NULL,name TEXT NOT NULL,
                rating INTEGER NOT NULL DEFAULT 0,flag INTEGER NOT NULL DEFAULT 0,
                keywords TEXT NOT NULL DEFAULT '',edits TEXT NOT NULL,metadata TEXT NOT NULL DEFAULT '{}',
                imported TEXT NOT NULL,label TEXT NOT NULL DEFAULT '',stack_id INTEGER,
                virtual_source INTEGER,extras TEXT NOT NULL DEFAULT '{}')
