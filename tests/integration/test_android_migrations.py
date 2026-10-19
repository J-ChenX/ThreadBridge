"""Check declared Room migrations against exported schemas and preserved cache data."""
import json
from pathlib import Path
import re
import sqlite3


def main():
    root = Path(__file__).resolve().parents[2]
    source = (root/'android/app/src/main/java/dev/threadbridge/Repository.kt').read_text()
    schemas = root/'android/app/schemas/dev.threadbridge.BridgeDatabase'
    database_source = (root/'android/app/src/main/java/dev/threadbridge/Database.kt').read_text()
    target_version = int(re.search(r'version=(\d+),exportSchema=true', database_source).group(1))
    assert target_version >= max(int(path.stem) for path in schemas.glob('*.json')), 'Room version regressed below a delivered schema'
    target = json.loads((schemas/f'{target_version}.json').read_text())['database']
    metadata_source = (root/'android/app/src/main/java/dev/threadbridge/DatabaseMigrations.kt').read_text()
    metadata_statements = [json.loads(sql) for sql in re.findall(r'db\.execSQL\(("(?:[^"\\]|\\.)*")\)', metadata_source)]
    assert len(metadata_statements) == 3
    assert 'MIGRATION_7_9' in source and 'MIGRATION_8_9' in source
    source += '\n' + metadata_source
    migrations = {}
    for start, end, body in re.findall(r'object\s*:\s*(?:androidx\.room\.migration\.)?Migration\((\d+),(\d+)\)\s*\{\s*override\s+fun\s+migrate\([^)]*\)\s*\{(.*?)\}\s*\}', source, re.S):
        migrations[int(start)] = (int(end), [json.loads(sql) for sql in re.findall(r'db\.execSQL\(("(?:[^"\\]|\\.)*")\)', body)], 'completeThreadMetadata(db)' in body)
    fixtures = [schemas/f'{version}.json' for version in range(1, target_version)]
    fixtures.append(root/'tests/fixtures/android/room-7-task-state.json')
    for fixture in fixtures:
        old = json.loads(fixture.read_text())['database']
        version = old['version']
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
        while current < target_version:
            next_version, statements, metadata = migrations[current]
            assert (statements or metadata) and next_version > current
            if metadata:
                columns = {row[1] for row in db.execute('PRAGMA table_info(threads)')}
                statements = [sql for sql in metadata_statements if re.search(r'ADD COLUMN `([^`]+)`', sql).group(1) not in columns]
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
        assert db.execute('SELECT messageRevision,messageActivityAt FROM threads').fetchone() == ((1, 1) if version >= 4 else (0, 0))
        db.close()
    print(f'Room migration: 1–8 and both delivered Room 7 shapes -> {target_version}; existing thread/task/display metadata, messages, drafts, pending requests and sync data preserved; schema version cannot regress')


if __name__ == '__main__':
    main()
