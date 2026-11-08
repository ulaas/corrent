"""Gives the screenshot profile a little history, as if someone had been watching: Sintel newest and half
watched (the home page's hero), Big Buck Bunny started, Caminandes' second episode under way, the first
episode, Elephants Dream and Cosmos Laundromat seen. Only ever run on the scratch profile's corrent.db.

    python -I scripts/site-media/stage.py <scratch>/app/corrent-data/corrent.db
"""

import sqlite3
import sys
import time

db = sqlite3.connect(sys.argv[1])
ids = {title: i for i, title in db.execute("SELECT id, title FROM items")}
user = db.execute("SELECT id FROM users ORDER BY id LIMIT 1").fetchone()[0]
now = int(time.time())
# Recently added: Sintel first, then Tears of Steel.
db.execute("UPDATE items SET added_at = added_at + 5 WHERE id = ?", (ids["Sintel"],))
db.execute("UPDATE items SET added_at = added_at + 3 WHERE id = ?", (ids["Tears of Steel"],))
for title, pos_ms, played, ago in [
    ("Sintel", 355_000, 0, 600),
    ("Big Buck Bunny", 73_500, 0, 5_400),
    ("Gran Dillama", 61_000, 0, 7_200),
    ("Llama Drama", 0, 1, 9_000),
    ("Elephants Dream", 0, 1, 90_000),
    ("Cosmos Laundromat", 0, 1, 86_400),
]:
    db.execute(
        "INSERT OR REPLACE INTO watch_state(user_id, item_id, position_ms, played, play_count, last_played_at) "
        "VALUES (?, ?, ?, ?, ?, ?)",
        (user, ids[title], pos_ms, played, played, now - ago),
    )
db.commit()
print("staged", sorted(ids))
