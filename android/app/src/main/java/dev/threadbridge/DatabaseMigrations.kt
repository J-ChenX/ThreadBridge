package dev.threadbridge

import androidx.room.migration.Migration
import androidx.sqlite.db.SupportSQLiteDatabase

// Both shapes of Room 7 were delivered: 1.0.1 had task state, while 1.0.3
// had display ownership. Room 8 (1.0.2) already has all three columns.
// Converge on Room 9 without dropping cached messages, drafts or request state.
private fun completeThreadMetadata(db:SupportSQLiteDatabase) {
 val columns=db.query("PRAGMA table_info(`threads`)").use { cursor->
  buildSet { while(cursor.moveToNext())add(cursor.getString(cursor.getColumnIndexOrThrow("name"))) }
 }
 if("taskStatus" !in columns)db.execSQL("ALTER TABLE `threads` ADD COLUMN `taskStatus` TEXT NOT NULL DEFAULT 'unknown'")
 if("taskTurnId" !in columns)db.execSQL("ALTER TABLE `threads` ADD COLUMN `taskTurnId` TEXT NOT NULL DEFAULT ''")
 if("displayHostId" !in columns)db.execSQL("ALTER TABLE `threads` ADD COLUMN `displayHostId` TEXT NOT NULL DEFAULT ''")
}

internal val MIGRATION_7_9=object:Migration(7,9) {
 override fun migrate(db:SupportSQLiteDatabase){completeThreadMetadata(db)}
}
internal val MIGRATION_8_9=object:Migration(8,9) {
 override fun migrate(db:SupportSQLiteDatabase){completeThreadMetadata(db)}
}
