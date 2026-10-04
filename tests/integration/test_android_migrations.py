"""Check declared Room migrations against exported schemas and preserved cache data."""
import json
from pathlib import Path
import re
import sqlite3


def main():
    root = Path(__file__).resolve().parents[2]
    source = (root/'android/app/src/main/java/dev/threadbridge/Repository.kt').read_text()
    schemas = root/'android/app/schemas/dev.threadbridge.BridgeDatabase'
    target = json.loads((schemas/'4.json').read_text())['database']
    migrations = {}
    for start, end, body in re.findall(r'object\s*:\s*Migration\((\d+),(\d+)\)\s*\{override fun migrate\([^)]*\)\{(.*?)\}\}', source, re.S):
        migrations[int(start)] = (int(end), [json.loads(sql) for sql in re.findall(r'db\.execSQL\(("(?:[^"\\]|\\.)*")\)', body)])
    for version in (1, 2, 3):
        old = json.loads((schemas/f'{version}.json').read_text())['database']
        db = sqlite3.connect(':memory:')
        snapshots = {}
        for entity in old['entities']:
            table = entity['tableName']
            db.execute(entity['createSql'].replace('${TABLE_NAME}', table))
            columns = [f['columnName'] for f in entity['fields']]
            values = [1 if f['affinity']=='INTEGER' else 'fixture' for f in entity['fields']]
            db.execute(f'INSERT INTO `{table}` VALUES({",".join("?" for _ in values)})', values)
            snapshots[table] = (columns, tuple(values))
        current = version
        while current < 4:
            next_version, statements = migrations[current]
            assert statements and next_version > current
            for statement in statements:
                db.execute(statement)
            current = next_version
        for entity in target['entities']:
            table = entity['tableName']
            columns = {row[1]: row for row in db.execute(f'PRAGMA table_info(`{table}`)')}
            assert set(columns) == {field['columnName'] for field in entity['fields']}
            for field in entity['fields']:
                column = columns[field['columnName']]
                assert column[2] == field['affinity']
                assert bool(column[3]) == field.get('notNull', False)
                assert column[4] == field.get('defaultValue')
            if table in snapshots:
                names, values = snapshots[table]
                quoted = ','.join(f'`{name}`' for name in names)
                assert db.execute(f'SELECT {quoted} FROM `{table}`').fetchone() == values
        assert db.execute('SELECT messageRevision,messageActivityAt FROM threads').fetchone() == (0, 0)
        db.close()
    print('Room migration: 1/2/3 -> 4 match exported columns and defaults; existing threads, messages, drafts, pending requests and sync data preserved')


if __name__ == '__main__':
    main()
