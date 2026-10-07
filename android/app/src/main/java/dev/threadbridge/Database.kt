package dev.threadbridge

import androidx.room.*
import kotlinx.coroutines.flow.Flow

@Entity(tableName="threads")
data class ThreadRow(@PrimaryKey val id:String, val title:String, val hostId:String, val status:String, val revision:String, val updated:Long, val canSend:Boolean, val online:Boolean, val historyCursor:String?, @ColumnInfo(defaultValue="0") val messageRevision:Long=0, @ColumnInfo(defaultValue="0") val messageActivityAt:Long=0, @ColumnInfo(defaultValue="''") val project:String="", @ColumnInfo(defaultValue="0") val localOnly:Boolean=false, @ColumnInfo(defaultValue="''") val projectName:String="", @ColumnInfo(defaultValue="0") val projectKnown:Boolean=false)
@Entity(tableName="messages", primaryKeys=["threadId","id"], indices=[Index(value=["threadId","ordinal"])])
data class MessageRow(val threadId:String,val id:String,val turnId:String,val role:String,val text:String,val version:String,val ordinal:Long,val characters:Int, @ColumnInfo(defaultValue="0") val timestamp:Long=0)
@Entity(tableName="drafts") data class Draft(@PrimaryKey val threadId:String,val text:String)
@Entity(tableName="pending") data class Pending(@PrimaryKey val requestId:String,val threadId:String,val payload:String,val status:String,val commandId:String?,val error:String?)
@Entity(tableName="sync") data class SyncState(@PrimaryKey val id:Int=1,val cursor:Long,@ColumnInfo(defaultValue="''") val generation:String="")
@Entity(tableName="hosts") data class HostRow(@PrimaryKey val id:String,val name:String,val online:Boolean)
@Entity(tableName="projects",primaryKeys=["hostId","path"]) data class ProjectRow(val hostId:String,val path:String, @ColumnInfo(defaultValue="''") val name:String="")
@Entity(tableName="chunks",primaryKeys=["threadId","messageId","version","offset"]) data class ChunkRow(val threadId:String,val messageId:String,val version:String,val offset:Int,val text:String)
@androidx.room.Dao interface BridgeDao {
 @Query("SELECT name FROM projects WHERE hostId=:host AND path=:path") suspend fun projectName(host:String,path:String):String?
 @Query("SELECT * FROM projects ORDER BY hostId,path") fun projects():Flow<List<ProjectRow>>
 @Insert(onConflict=OnConflictStrategy.REPLACE) suspend fun putProjects(rows:List<ProjectRow>)
 @Query("DELETE FROM projects") suspend fun clearProjects()
 @Query("SELECT * FROM hosts ORDER BY CASE lower(name) WHEN 'echova' THEN 0 WHEN 'lerrem' THEN 1 WHEN 'nix' THEN 2 ELSE 3 END,name") fun hosts():Flow<List<HostRow>>
 @Insert(onConflict=OnConflictStrategy.REPLACE) suspend fun putHosts(rows:List<HostRow>)
 @Query("DELETE FROM hosts") suspend fun clearHosts()
 @Query("SELECT generation FROM sync WHERE id=1") suspend fun generation():String?
 @Query("DELETE FROM drafts") suspend fun clearDrafts()
 @Query("DELETE FROM pending") suspend fun clearPending()
 @Query("DELETE FROM drafts WHERE threadId=:id") suspend fun deleteDraft(id:String)
 @Query("DELETE FROM pending WHERE threadId=:id") suspend fun deletePending(id:String)
 @Query("SELECT * FROM threads") suspend fun allThreads():List<ThreadRow>
 @Query("SELECT id FROM threads") suspend fun threadIds():List<String>
 @Query("DELETE FROM threads WHERE id=:id") suspend fun deleteThread(id:String)
 @Query("DELETE FROM messages WHERE threadId=:id") suspend fun deleteThreadMessages(id:String)
 @Query("DELETE FROM chunks WHERE threadId=:id") suspend fun deleteThreadChunks(id:String)
 @Query("DELETE FROM chunks WHERE threadId=:thread AND messageId=:id") suspend fun deleteMessageChunks(thread:String,id:String)
 @Query("DELETE FROM messages WHERE threadId=:thread AND id=:id") suspend fun deleteMessage(thread:String,id:String)
 @Query("DELETE FROM threads") suspend fun clearThreads()
 @Query("DELETE FROM chunks") suspend fun clearChunks()
 @Query("DELETE FROM messages") suspend fun clearMessages()
 @Query("SELECT * FROM threads ORDER BY MAX(updated*1000,messageActivityAt) DESC,id") fun threads():Flow<List<ThreadRow>>
 @Query("SELECT * FROM threads WHERE id=:id") suspend fun thread(id:String):ThreadRow?
 @Query("SELECT * FROM messages WHERE threadId=:id AND (ordinal<:before OR (ordinal=:before AND id<:beforeId)) ORDER BY ordinal DESC,id DESC LIMIT :limit") fun messages(id:String,before:Long,beforeId:String,limit:Int=100):Flow<List<MessageRow>>
 @Query("SELECT * FROM drafts WHERE threadId=:id") suspend fun draft(id:String):Draft?
 @Insert(onConflict=OnConflictStrategy.REPLACE) suspend fun putThreads(rows:List<ThreadRow>)
 @Insert(onConflict=OnConflictStrategy.REPLACE) suspend fun putMessages(rows:List<MessageRow>)
 @Query("UPDATE drafts SET text='' WHERE threadId=:id AND text=:sent") suspend fun clearDraftIfMatches(id:String,sent:String)
 @Query("SELECT * FROM chunks WHERE threadId=:thread AND messageId=:message AND version=:version AND offset=:offset") suspend fun chunk(thread:String,message:String,version:String,offset:Int):ChunkRow?
 @Insert(onConflict=OnConflictStrategy.REPLACE) suspend fun putChunk(row:ChunkRow)
 @Query("DELETE FROM chunks WHERE rowid NOT IN (SELECT rowid FROM chunks ORDER BY rowid DESC LIMIT 128)") suspend fun pruneChunks()
 @Insert(onConflict=OnConflictStrategy.REPLACE) suspend fun putDraft(row:Draft)
 @Insert(onConflict=OnConflictStrategy.REPLACE) suspend fun putPending(row:Pending)
 @Query("SELECT * FROM pending WHERE threadId=:id ORDER BY rowid DESC LIMIT 1") fun pending(id:String):Flow<Pending?>
 @Query("SELECT * FROM pending WHERE status IN ('submitting','accepted','dispatching','upstream_queued','unknown') LIMIT 128") suspend fun unresolved():List<Pending>
 @Query("SELECT p.* FROM pending p JOIN threads t ON p.threadId=t.id WHERE t.localOnly=1 LIMIT 128") suspend fun localCreations():List<Pending>
 @Query("SELECT * FROM pending WHERE requestId=:id") suspend fun pendingById(id:String):Pending?
 @Insert(onConflict=OnConflictStrategy.REPLACE) suspend fun putSync(row:SyncState)
 @Query("SELECT cursor FROM sync WHERE id=1") suspend fun cursor():Long?
 @Query("DELETE FROM chunks WHERE NOT EXISTS (SELECT 1 FROM messages WHERE messages.threadId=chunks.threadId AND messages.id=chunks.messageId AND messages.version=chunks.version)") suspend fun pruneOldChunks()
 @Query("SELECT COALESCE(SUM(length(CAST(text AS BLOB))),0) FROM messages") suspend fun messageBytes():Long
 @Query("DELETE FROM messages WHERE rowid IN (SELECT rowid FROM messages ORDER BY rowid ASC LIMIT 100)") suspend fun evictMessages()
 @Query("DELETE FROM messages WHERE rowid IN (SELECT rowid FROM messages ORDER BY rowid ASC LIMIT MAX(0,(SELECT count(*) FROM messages)-5000))") suspend fun prune()
}
@Database(entities=[ThreadRow::class,MessageRow::class,Draft::class,Pending::class,SyncState::class,ChunkRow::class,HostRow::class,ProjectRow::class],version=6,exportSchema=true)
abstract class BridgeDatabase:RoomDatabase(){abstract fun dao():BridgeDao}
