package dev.threadbridge

import android.content.Context
import androidx.room.Room
import androidx.room.withTransaction
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.channels.Channel
import androidx.room.migration.Migration
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import okhttp3.*
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject
import java.util.UUID
import java.util.concurrent.TimeUnit

class ApiException(val code:Int,val reason:String):Exception(reason)
data class MessageCursor(val ordinal:Long,val id:String)
class Repository(context:Context) {
 companion object { @Volatile private var instance:Repository?=null; fun get(context:Context):Repository=instance?:synchronized(this){instance?:Repository(context.applicationContext).also{instance=it}} }
 val db=Room.databaseBuilder(context,BridgeDatabase::class.java,"threadbridge.sqlite").addMigrations(object:Migration(1,2){override fun migrate(db:androidx.sqlite.db.SupportSQLiteDatabase){db.execSQL("CREATE TABLE IF NOT EXISTS `chunks` (`threadId` TEXT NOT NULL, `messageId` TEXT NOT NULL, `version` TEXT NOT NULL, `offset` INTEGER NOT NULL, `text` TEXT NOT NULL, PRIMARY KEY(`threadId`,`messageId`,`version`,`offset`))")}},object:Migration(2,3){override fun migrate(db:androidx.sqlite.db.SupportSQLiteDatabase){db.execSQL("ALTER TABLE `sync` ADD COLUMN `generation` TEXT NOT NULL DEFAULT ''");db.execSQL("CREATE TABLE IF NOT EXISTS `hosts` (`id` TEXT NOT NULL, `name` TEXT NOT NULL, `online` INTEGER NOT NULL, PRIMARY KEY(`id`))")}},object:Migration(3,4){override fun migrate(db:androidx.sqlite.db.SupportSQLiteDatabase){db.execSQL("ALTER TABLE `threads` ADD COLUMN `messageRevision` INTEGER NOT NULL DEFAULT 0");db.execSQL("ALTER TABLE `threads` ADD COLUMN `messageActivityAt` INTEGER NOT NULL DEFAULT 0")}},object:Migration(4,5){override fun migrate(db:androidx.sqlite.db.SupportSQLiteDatabase){db.execSQL("ALTER TABLE `threads` ADD COLUMN `project` TEXT NOT NULL DEFAULT ''");db.execSQL("ALTER TABLE `threads` ADD COLUMN `localOnly` INTEGER NOT NULL DEFAULT 0");db.execSQL("ALTER TABLE `messages` ADD COLUMN `timestamp` INTEGER NOT NULL DEFAULT 0");db.execSQL("CREATE TABLE IF NOT EXISTS `projects` (`hostId` TEXT NOT NULL, `path` TEXT NOT NULL, PRIMARY KEY(`hostId`,`path`))");db.execSQL("UPDATE `messages` SET `timestamp`=`ordinal` WHERE `ordinal`>0")}},object:Migration(5,6){override fun migrate(db:androidx.sqlite.db.SupportSQLiteDatabase){db.execSQL("ALTER TABLE `threads` ADD COLUMN `projectName` TEXT NOT NULL DEFAULT ''");db.execSQL("ALTER TABLE `projects` ADD COLUMN `name` TEXT NOT NULL DEFAULT ''");db.execSQL("ALTER TABLE `threads` ADD COLUMN `projectKnown` INTEGER NOT NULL DEFAULT 0")}}).build()
 val dao=db.dao();val credentials=Credentials(context)
 internal val library=ConversationLibrary(context)
 internal val searchState=kotlinx.coroutines.flow.MutableStateFlow(ConversationSearchState())
 val redirects=kotlinx.coroutines.flow.MutableStateFlow<Map<String,String>>(emptyMap())
 internal val readState=ConversationReadState(context)
 private val imageCache=java.io.File(context.cacheDir,"conversation-images")
 private val imageLock=Mutex()
 private val imageMemory=object:android.util.LruCache<String,android.graphics.Bitmap>(8*1024*1024){override fun sizeOf(key:String,value:android.graphics.Bitmap)=value.allocationByteCount}
 private val capturePrefs=context.getSharedPreferences("capture_status",Context.MODE_PRIVATE)
 val captureWarning=MutableStateFlow<String?>(capturePrefs.getString("warning",null))
 val connection=MutableStateFlow("未连接")
 val connectionIssue=MutableStateFlow<String?>(null)
 @Volatile private var httpHealthy=false
 @Volatile private var connectedCount=0
 private val pollingFallback=MutableStateFlow(false)
 private fun polling()=pollingFallback.value||PublicTestConnection.usePolling(credentials.server,BuildConfig.PUBLIC_TEST_SERVER)
 private fun publishConnected(){if(httpHealthy)connection.value=connectedLabel(connectedCount,polling())}
 val collection=MutableStateFlow("")
 private val client=OkHttpClient.Builder().connectTimeout(8,TimeUnit.SECONDS).readTimeout(15,TimeUnit.SECONDS).callTimeout(20,TimeUnit.SECONDS).followRedirects(false).followSslRedirects(false).build()
 private var socket:WebSocket?=null
 private val sessionGate=SyncSessionGate()
 private val syncLock=Mutex()
 private val ledgerLock=Mutex()
 private var foregroundScope:CoroutineScope?=null
 private var session:Job?=null
 private val draftScope=CoroutineScope(SupervisorJob()+Dispatchers.IO)
 private data class DraftWrite(val draft:Draft?=null,val barrier:CompletableDeferred<Unit>?=null,val generation:Long=0)
 private val draftPreview=DraftPreviewCache()
 private val draftWrites=Channel<DraftWrite>(64)
 init { draftScope.launch { for(write in draftWrites){try{write.draft?.let{sessionGate.run(write.generation){val target=redirects.value[it.threadId]?:it.threadId;if(dao.thread(target)!=null)dao.putDraft(it.copy(threadId=target))};draftPreview.committed(it.threadId,write.generation,it.text)}}catch(_:CancellationException){}finally{write.barrier?.complete(Unit)}} } }
 fun saveDraft(id:String,text:String){val generation=sessionGate.current();draftPreview.put(id,generation,text);val write=DraftWrite(Draft(id,text),generation=generation);if(draftWrites.trySend(write).isFailure)draftScope.launch(start=CoroutineStart.UNDISPATCHED){draftWrites.send(write)}}
 private suspend fun flushDrafts(){val done=CompletableDeferred<Unit>();draftWrites.send(DraftWrite(barrier=done));done.await()}
 suspend fun draft(id:String):String{
  val generation=sessionGate.current()
  // A local read does not need to wait behind network requests. Validate the
  // collection generation after Room returns; queued writes stay serialized.
  val text=draftPreview.get(id,generation)?:dao.draft(id)?.text?:""
  if(generation!=sessionGate.current())throw CancellationException("Connection changed")
  return draftPreview.get(id,generation)?:text
 }
 private suspend fun updatePending(p:Pending):Pending=ledgerLock.withLock{
  check(dao.thread(p.threadId)!=null){"此对话已从同步服务删除"}
  val current=dao.pendingById(p.requestId)
  val updated=if(current!=null&&!SyncPolicy.acceptUpdate(current.status,p.status))current else p
  dao.putPending(updated);updated
 }
 var selected:String?=null
 fun validateServer(value:String,lan:Boolean):String=ConnectionConfiguration.server(value,lan)
 private suspend fun request(path:String,body:JSONObject?=null,server:String=credentials.server,token:String=credentials.token(),maxBytes:Int=1024*1024,lan:Boolean=credentials.lan || token.isEmpty()):JSONObject=withContext(Dispatchers.IO){
  validateServer(server,lan)
  val b=Request.Builder().url(server+path);if(token.isNotEmpty())b.header("Authorization","Bearer $token")
  if(body!=null)b.post(body.toString().toRequestBody("application/json".toMediaType()))
  client.newCall(b.build()).execute().use{r->val input=r.body?.byteStream()?:throw InvalidApiResponse();val output=java.io.ByteArrayOutputStream();val buffer=ByteArray(8192);while(true){val n=input.read(buffer);if(n<0)break;if(output.size()+n>maxBytes)throw ResponseTooLarge();output.write(buffer,0,n)};val bytes=output.toByteArray();val json=try{JSONObject(String(bytes,Charsets.UTF_8))}catch(_:org.json.JSONException){if(!r.isSuccessful)throw ApiException(r.code,"non_json_response");throw InvalidApiResponse()};if(!r.isSuccessful)throw ApiException(r.code,json.optString("error","连接失败"));json}
 }
 suspend fun pair(server:String,code:String,lan:Boolean){
  stopSession()
  try {sessionGate.reset{val url=validateServer(server,lan);val r=request("/v1/pair",JSONObject().put("code",code.trim()).put("name","Android 主手机"),url,"");require(r.getInt("protocol")==1){"服务端协议版本不兼容"};withContext(Dispatchers.IO){db.clearAllTables();imageMemory.evictAll();imageCache.deleteRecursively()};readState.clear();library.clear();searchState.value=ConversationSearchState();redirects.value=emptyMap();draftPreview.clear();credentials.save(url,r.getString("token"),lan)}}
  finally { // reset has released its mutex and advanced the generation before workers start.
   foregroundScope?.takeIf{it.isActive}?.let{connect(it)}
  }
 }
 suspend fun configureConnection(server:String,key:String,lan:Boolean){
  val endpoint=validateServer(server,lan)
  val token=ConnectionConfiguration.key(key,credentials.token())
  stopSession()
  try {
   // Flush in-flight edits before taking the sync gate; queued writes also use it.
   flushDrafts()
   sessionGate.run {
    val response=try{request("/v1/hosts",server=endpoint,token=token,lan=lan)}catch(e:CancellationException){throw e}catch(e:Exception){throw IllegalArgumentException(if(e is ApiException&&e.code in listOf(401,403))"连接密钥无效或没有手机访问权限"else connectionFailure(e))}
    val incoming=response.optString("collection_generation")
    require(response.optJSONArray("hosts")!=null&&incoming.isNotBlank()){ "该地址未返回兼容的同步服务" }
    require(ConnectionConfiguration.sameCollection(dao.generation().orEmpty(),incoming)){ "该地址对应不同的同步库，请通过重新配对切换" }
    credentials.save(endpoint,token,lan)
    sessionGate.invalidateWithinOperation();draftPreview.clear()
    httpHealthy=false;connectionIssue.value=null
   }
  }finally{foregroundScope?.takeIf{it.isActive}?.let{connect(it)}}
 }
 private suspend fun migratePublicTestConnection(){
  val token=credentials.token()
  if(!PublicTestConnection.shouldMigrate(credentials.server,token,BuildConfig.PUBLIC_TEST_PREVIOUS_SERVER,BuildConfig.PUBLIC_TEST_SERVER,BuildConfig.PUBLIC_TEST_HOST))return
  try {
   val response=request("/v1/hosts",server=BuildConfig.PUBLIC_TEST_SERVER,token=token)
   val hosts=response.getJSONArray("hosts")
   val identities=(0 until hosts.length()).map{hosts.getJSONObject(it).getString("id")}
   check(PublicTestConnection.matchesHost(identities,BuildConfig.PUBLIC_TEST_HOST)){"公网连接与已配对电脑不一致"}
   // This is the same Hub through another transport. Preserve the token, drafts,
   // cache, cursor and immutable pending requests; do not create a new pairing.
   credentials.save(BuildConfig.PUBLIC_TEST_SERVER,token,false)
   socket?.cancel()
  }catch(e:CancellationException){throw e}catch(_:Exception){/* Keep the existing connection and retry later. */}
 }
 private suspend fun clearCachedContent(id:String){withContext(Dispatchers.IO){imageMemory.snapshot().keys.filter{it.startsWith("$id:")}.forEach{imageMemory.remove(it)};imageCache.listFiles()?.filter{it.name.contains("-$id-")}?.forEach{it.delete()}};dao.deleteThreadMessages(id);dao.deleteThreadChunks(id)}
 private suspend fun deleteCachedThread(id:String){clearCachedContent(id);dao.deleteThread(id);dao.deleteDraft(id);dao.deletePending(id)}
 private suspend fun syncInternal(){syncLock.withLock{try{
  migratePublicTestConnection()
  val previous=dao.cursor()?:0
  val changes=request("/v1/events?after=$previous")
  val r=request("/v1/threads")
  val epoch=r.getString("collection_generation")
  check(changes.getString("collection_generation")==epoch){"同步已重置，请稍后重试"}
  val hostResponse=request("/v1/hosts")
  check(hostResponse.getString("collection_generation")==epoch){"同步已重置，请稍后重试"}
  val ha=hostResponse.getJSONArray("hosts")
  val hosts=(0 until ha.length()).map{val h=ha.getJSONObject(it);HostRow(h.getString("id"),h.getString("name"),h.getBoolean("online"))}
  val projects=mutableListOf<ProjectRow>();for(i in 0 until ha.length()){val h=ha.getJSONObject(i);val paths=h.optJSONArray("projects")?:continue;for(j in 0 until minOf(paths.length(),1000))projects+=ProjectRow(h.getString("id"),paths.getString(j),h.optJSONArray("project_details")?.let{a->(0 until a.length()).map{a.getJSONObject(it)}.find{it.optString("path")==paths.getString(j)}?.optString("name")}.orEmpty())}
  val rows=mutableListOf<ThreadRow>();var page=r;var count=0
  while(true){
   check(page.getString("collection_generation")==epoch){"同步已重置，请稍后重试"}
   val a=page.getJSONArray("threads")
   for(i in 0 until a.length()){val t=a.getJSONObject(i);rows+=ThreadRow(t.getString("id"),t.getString("title"),t.getString("host_id"),t.getString("status"),t.getString("revision"),t.getLong("updated_at"),t.getBoolean("can_send"),t.getBoolean("host_online"),if(t.isNull("history_cursor"))null else t.getString("history_cursor"),t.optLong("message_revision",0),t.optLong("message_activity_at",0),t.optString("project",""),projectName=t.optString("project_name",""),projectKnown=t.optBoolean("project_known",t.optString("project").isNotEmpty()))}
   count++;if(page.isNull("next_offset"))break
   val next=page.getLong("next_offset");require(next<=100000&&next==(count*50).toLong()){"对话列表分页异常"};page=request("/v1/threads?offset=$next")
  }
  val reset=dao.generation()!=epoch
  if(reset){library.clear();searchState.value=ConversationSearchState();redirects.value=emptyMap()}
  val local=if(reset)emptyList() else dao.allThreads().filter{it.localOnly}
  library.reconcile((rows.map{it.id}+local.map{it.id}).toSet())
  rows.removeAll{library.expired(it)}
  rows.addAll(local.map{it.copy(online=hosts.find{h->h.id==it.hostId}?.online?:false)})
  val liveIds=rows.map{it.id}.toSet()
  if(reset){withContext(Dispatchers.IO){imageMemory.evictAll();imageCache.deleteRecursively()};sessionGate.invalidateWithinOperation();draftPreview.clear();selected=null;captureWarning.value=null;capturePrefs.edit().clear().apply()}
  db.withTransaction{
   if(reset){dao.clearMessages();dao.clearChunks();dao.clearThreads();dao.clearDrafts();dao.clearPending();dao.clearHosts()}
   if(changes.optBoolean("snapshot_required",false)){dao.clearMessages();dao.clearChunks();dao.clearThreads()}
   val events=changes.optJSONArray("events")
   if(events!=null)for(i in 0 until events.length()){val event=events.getJSONObject(i);val kind=event.getString("kind");val thread=event.getString("thread_id");if(kind=="delete_thread")deleteCachedThread(thread)else if(kind.startsWith("delete_message:")){dao.deleteMessage(thread,kind.removePrefix("delete_message:"));dao.deleteMessageChunks(thread,kind.removePrefix("delete_message:"))}}
   for(id in dao.threadIds())if(id !in liveIds)deleteCachedThread(id)
   dao.clearHosts();dao.putHosts(hosts);dao.clearProjects();dao.putProjects(projects);dao.putThreads(rows)
   dao.putSync(SyncState(cursor=if(changes.optBoolean("snapshot_required",false)||reset)r.getLong("cursor")else changes.optLong("cursor",previous),generation=epoch))
  }
  cleanupLocal()
  readState.observe(epoch,dao.allThreads())
  collection.value=epoch
  loadCaptureHealth();reconcileInternal();selected?.let{if(dao.thread(it)!=null)loadMessagesInternal(it)else selected=null};connectedCount=rows.size;httpHealthy=true;connectionIssue.value=null;publishConnected()
 }catch(e:Exception){if(e is CancellationException)throw e;httpHealthy=false;connectionIssue.value=connectionFailure(e);connection.value=if(e is ApiException&&e.code==401)"凭证已失效，请重新配对"else"离线 · 显示已缓存内容";throw e}}}
 suspend fun deleteThread(id:String)=sessionGate.run{
  if(dao.thread(id)?.localOnly!=true)request("/v1/threads/$id/delete",JSONObject())
  library.remove(id)
  sessionGate.invalidateWithinOperation()
  db.withTransaction{deleteCachedThread(id)}
  if(selected==id)selected=null
 }
 private suspend fun checkEpoch(response:JSONObject){check(response.getString("collection_generation")==dao.generation()){"同步已重置，请刷新对话"}}
 private suspend fun loadCaptureHealth(){
  val warning=try {
   val response=request("/v1/capture-health");val hosts=response.getJSONArray("hosts");val now=response.getLong("server_time")
   var count=0;var inputFailures=0;var overflow=false;var projectionError=false;var stale=false;var detail:String?=null
   for(i in 0 until hosts.length()){
    val host=hosts.getJSONObject(i);val status=host.getJSONObject("status");val failures=status.optJSONObject("failures")
    if(failures!=null)for(key in failures.keys()){if(failures.getJSONObject(key).optString("reason")=="user_input_capture_failed")inputFailures++ else count++};overflow=overflow||status.optBoolean("overflow");projectionError=projectionError||status.has("projection_error");stale=stale||now-host.getLong("checked_at")>30
    if(failures!=null&&failures.length()>0){val failure=failures.getJSONObject(failures.keys().next());detail="会话 ${failure.optString("thread_id").take(8)}：${CapturePresentation.reason(failure.optString("reason"))}"}
   }
   CapturePresentation.warning(count,overflow,projectionError,stale,hosts.length()==0,inputFailures)?.let{it+(detail?.let{d->"\n$d"}?:"")}
  }catch(e:Exception){if(e is CancellationException)throw e;"无法核实电脑回复保存状态；已有内容仍可读取。"}
  captureWarning.value=warning;capturePrefs.edit().putString("warning",warning).apply()
 }
 private suspend fun loadMessagesInternal(id:String,before:Long?=null,beforeId:String?=null):MessageCursor?{if(dao.thread(id)?.localOnly==true)return null;val r=request("/v1/threads/$id/messages"+(before?.let{"?before=$it"+(beforeId?.let{"&before_id=${java.net.URLEncoder.encode(it,"UTF-8")}"}?:"")}?:""));checkEpoch(r);if(dao.thread(id)==null)return null;val a=r.getJSONArray("messages");val rows=(0 until a.length()).map{val m=a.getJSONObject(it);MessageRow(id,m.getString("id"),m.getString("turn_id"),m.getString("role"),m.getString("text"),m.getString("version"),m.getLong("ordinal"),m.getInt("characters"),m.optLong("timestamp_ms",m.getLong("ordinal")))};db.withTransaction{dao.putMessages(rows);dao.prune();dao.pruneOldChunks();while(dao.messageBytes()>16*1024*1024)dao.evictMessages()};return if(r.has("next_before")){if(r.isNull("next_before"))null else MessageCursor(r.getLong("next_before"),r.getString("next_before_id"))}else rows.lastOrNull()?.let{MessageCursor(it.ordinal,it.id)}}
 private suspend fun chunkInternal(m:MessageRow,offset:Int):String {
  dao.chunk(m.threadId,m.id,m.version,offset)?.let{return it.text}
  val response=request("/v1/threads/${m.threadId}/messages/${m.id}/body?version=${m.version}&offset=$offset");checkEpoch(response);check(dao.thread(m.threadId)!=null);val text=response.getString("text")
  dao.putChunk(ChunkRow(m.threadId,m.id,m.version,offset,text));dao.pruneChunks();return text
 }
 private suspend fun submitInternal(thread:ThreadRow,text:String,history:Boolean=false):Pending {
  check(dao.thread(thread.id)!=null){"此对话已从同步服务删除"}
  if(thread.localOnly)return submitNew(thread,text)
  val id=UUID.randomUUID().toString();val p=JSONObject().put("request_id",id).put("thread_id",thread.id).put("text",text).put("expected_revision",thread.revision).put("created_at",System.currentTimeMillis()/1000).put("kind",if(history)"history" else "send").put("cursor",thread.historyCursor?:JSONObject.NULL)
  val snapshot=Pending(id,thread.id,p.toString(),"submitting",null,null);dao.putPending(snapshot)
  return try{val r=request("/v1/commands",p);val updated=snapshot.copy(status=r.getString("status"),commandId=r.getString("id"));val committed=updatePending(updated);if(!history)dao.clearDraftIfMatches(thread.id,text);committed}catch(e:ApiException){if(e.code in listOf(400,401,403,409,413,429)){val failed=snapshot.copy(status="rejected",error=e.reason);updatePending(failed)}else{snapshot}}catch(e:Exception){if(e is CancellationException)throw e;snapshot}
 }
 private suspend fun reconcileInternal(){for(p in (dao.unresolved()+dao.localCreations()).distinctBy{it.requestId}){try{val r=request("/v1/commands/${p.requestId}");val updated=updatePending(p.copy(status=r.getString("status"),commandId=r.getString("id"),error=if(r.isNull("error"))null else r.getString("error")));adoptCreated(updated,r)}catch(e:Exception){if(e is CancellationException)throw e;/* Unknown submission stays immutable. */}}}
 private suspend fun adoptCreated(p:Pending,response:JSONObject){
  val target=response.optString("result_thread_id","");if(target.isBlank()||target=="null")return
  val local=dao.thread(p.threadId)?.takeIf{it.localOnly}?:return
  if(dao.thread(target)==null)return // Next list sync will expose the persisted projection.
  db.withTransaction{val draft=draftPreview.get(local.id,sessionGate.current())?:dao.draft(local.id)?.text.orEmpty();if(draft.isNotEmpty())dao.putDraft(Draft(target,draft));dao.putPending(p.copy(threadId=target));dao.deleteDraft(local.id);dao.deleteThread(local.id)}
  library.move(local.id,target);if(selected==local.id)selected=target;redirects.value=redirects.value+(local.id to target)
 }
 private suspend fun submitNew(thread:ThreadRow,text:String):Pending{
  val id=UUID.randomUUID().toString();val payload=JSONObject().put("request_id",id).put("host_id",thread.hostId).put("project",thread.project).put("text",text).put("created_at",System.currentTimeMillis()/1000).put("kind","create")
  val pending=Pending(id,thread.id,payload.toString(),"submitting",null,null);dao.putPending(pending)
  return try{val r=request("/v1/threads/create",payload);val result=updatePending(pending.copy(status=r.getString("status"),commandId=r.getString("id")));dao.clearDraftIfMatches(thread.id,text);result}catch(e:ApiException){if(e.code in 400..499)updatePending(pending.copy(status="rejected",error=e.message))else pending}catch(e:Exception){if(e is CancellationException)throw e;pending}
 }
 suspend fun searchTitles(query:String)=sessionGate.run{
  if(query.isBlank()){searchState.value=ConversationSearchState();return@run}
  val local=dao.allThreads().filter{titleMatches(it.title,query)}
  searchState.value=ConversationSearchState(local,loading=true)
  try{
   val rows=mutableListOf<ThreadRow>();var offset=0;var more=false
   do{val r=request("/v1/threads?q=${java.net.URLEncoder.encode(query,"UTF-8")}&offset=$offset");checkEpoch(r);val a=r.getJSONArray("threads")
    for(i in 0 until a.length()){val t=a.getJSONObject(i);val row=ThreadRow(t.getString("id"),t.getString("title"),t.getString("host_id"),t.getString("status"),t.getString("revision"),t.getLong("updated_at"),t.getBoolean("can_send"),t.getBoolean("host_online"),if(t.isNull("history_cursor"))null else t.getString("history_cursor"),t.optLong("message_revision",0),t.optLong("message_activity_at",0),t.optString("project",""),projectName=t.optString("project_name",""),projectKnown=t.optBoolean("project_known",t.optString("project").isNotEmpty()));if(titleMatches(row.title,query))rows+=row}
    more=!r.isNull("next_offset");if(more)offset=r.getInt("next_offset")
   }while(more&&offset<1000)
   searchState.value=ConversationSearchState((rows+local).distinctBy{it.id},more=more)
  }catch(e:Exception){if(e is CancellationException)throw e;searchState.value=ConversationSearchState(local,offline=true)}
 }
 suspend fun restoreSearchedThread(thread:ThreadRow)=sessionGate.run{library.restore(thread.id);dao.putThreads(listOf(thread));readState.markRead(thread)}
 suspend fun createBlank(host:HostRow,project:String):String=sessionGate.run{
  val id="local-"+UUID.randomUUID();dao.putThreads(listOf(ThreadRow(id,"新对话",host.id,"idle","",System.currentTimeMillis()/1000,true,host.online,null,project=project,localOnly=true,projectName=dao.projectName(host.id,project).orEmpty(),projectKnown=true)));id
 }
 private suspend fun cleanupLocal(){
  val now=System.currentTimeMillis();if(!library.cleanupDue(now))return
  val protected=library.marks.value.let{it.pinned+it.favorites};val pending=dao.unresolved().map{it.threadId}.toSet()
  for(thread in dao.allThreads())if(canExpire(thread,now,protected,draftPreview.get(thread.id,sessionGate.current())?:dao.draft(thread.id)?.text,thread.id in pending,selected)){
   db.withTransaction{deleteCachedThread(thread.id)};if(!thread.localOnly)library.expire(thread)
  };library.cleaned(now)
 }
 private suspend fun cancelInternal(p:Pending){check(dao.pendingById(p.requestId)!=null);val id=p.commandId?:return;val r=request("/v1/commands/$id/cancel",JSONObject());updatePending(p.copy(status=r.getString("status")))}
 private suspend fun retryPendingInternal(p:Pending){require(p.status=="submitting");check(dao.pendingById(p.requestId)!=null);try{val found=request("/v1/commands/${p.requestId}");updatePending(p.copy(status=found.getString("status"),commandId=found.getString("id")));return}catch(e:ApiException){if(e.code!=404)throw e}
  val json=JSONObject(p.payload);if(System.currentTimeMillis()/1000-json.getLong("created_at")>120){updatePending(p.copy(status="expired",error="提交窗口已过期，未查询到回执"));return};val r=request(if(json.optString("kind")=="create")"/v1/threads/create"else"/v1/commands",json);updatePending(p.copy(status=r.getString("status"),commandId=r.getString("id")))
 }
 suspend fun userImage(m:MessageRow,image:String,maxDimension:Int=720):android.graphics.Bitmap=sessionGate.run{withContext(Dispatchers.IO){imageLock.withLock{
  require(image.matches(Regex("[a-f0-9]{64}")))
  require(maxDimension in 256..1600)
  check(dao.thread(m.threadId)!=null){"对话已删除"}
  val namespace=java.security.MessageDigest.getInstance("SHA-256").digest((credentials.server+credentials.token()+dao.generation()).toByteArray()).joinToString(""){"%02x".format(it)}
  val memoryKey="${m.threadId}:$namespace:$image:$maxDimension"
  imageMemory.get(memoryKey)?.let{return@withLock it}
  val file=java.io.File(imageCache,"$namespace-${m.threadId}-$image")
  val bytes=if(file.exists())file.readBytes()else{
   val message=java.net.URLEncoder.encode(m.id,"UTF-8")
   val r=request("/v1/threads/${m.threadId}/messages/$message/images/$image",maxBytes=3*1024*1024);checkEpoch(r)
   val data=android.util.Base64.decode(r.getString("base64"),android.util.Base64.DEFAULT)
   require(data.size<=2*1024*1024)
   val digest=java.security.MessageDigest.getInstance("SHA-256").digest(data).joinToString(""){"%02x".format(it)}
   check(digest==image){"图片校验失败"}
   imageCache.mkdirs()
   var used=imageCache.listFiles()?.sumOf{it.length()}?:0
   for(old in imageCache.listFiles()?.sortedBy{it.lastModified()}?:emptyList()){if(used+data.size<=64*1024*1024)break;val size=old.length();if(old.delete())used-=size}
   file.writeBytes(data);data
  }
  val options=android.graphics.BitmapFactory.Options().apply{inJustDecodeBounds=true}
  android.graphics.BitmapFactory.decodeByteArray(bytes,0,bytes.size,options)
  require(options.outWidth>0&&options.outHeight>0){"无法读取图片"}
  options.inJustDecodeBounds=false;options.inSampleSize=bitmapSample(options.outWidth,options.outHeight,maxDimension)
  (android.graphics.BitmapFactory.decodeByteArray(bytes,0,bytes.size,options)?:error("无法读取图片")).also{imageMemory.put(memoryKey,it)}
 }}}
 suspend fun sync()=sessionGate.run{syncInternal()}
 suspend fun loadMessages(id:String,before:Long?=null,beforeId:String?=null)=sessionGate.run{loadMessagesInternal(id,before,beforeId)}
 suspend fun chunk(m:MessageRow,offset:Int)=sessionGate.run{chunkInternal(m,offset)}
 suspend fun submit(thread:ThreadRow,text:String,history:Boolean=false):Pending {val generation=sessionGate.current();flushDrafts();return sessionGate.run(generation){submitInternal(thread,text,history)}}
 suspend fun cancel(p:Pending)=sessionGate.run{cancelInternal(p)}
 suspend fun retryPending(p:Pending)=sessionGate.run{retryPendingInternal(p)}
 fun connect(scope:CoroutineScope){
  stopSession();foregroundScope=scope;if(credentials.token().isEmpty())return
  httpHealthy=false;pollingFallback.value=false;connectionIssue.value=null;connection.value="正在连接"
  session=scope.launch {
   sessionGate.run{cleanupLocal()}
   val wake=Channel<Unit>(Channel.CONFLATED)
   launch { for(signal in wake){try{sync()}catch(e:CancellationException){throw e}catch(_:Exception){}} }
   launch { pollingFallback.collectLatest { while(isActive){wake.trySend(Unit);delay(if(polling())3000 else 15000)} } }
   val currentSession=currentCoroutineContext()[Job]!!
   var failures=0
   while(isActive){
    // This cpolar test transport accepts authenticated HTTP but drops the
    // WebSocket authorization. Keep the foreground polling path authenticated.
    if(PublicTestConnection.usePolling(credentials.server,BuildConfig.PUBLIC_TEST_SERVER)){delay(3000);continue}
    val ended=CompletableDeferred<Unit>()
    try {
     val cursor=dao.cursor()?:0
     val endpoint=credentials.server
     val url=(endpoint+"/v1/events/ws?after=$cursor").replaceFirst("https://","wss://").replaceFirst("http://","ws://")
     val ws=client.newWebSocket(Request.Builder().url(url).header("Authorization","Bearer ${credentials.token()}").build(),object:WebSocketListener(){
      override fun onOpen(webSocket:WebSocket,response:Response){scope.launch{if(currentSession.isActive&&endpoint==credentials.server){failures=0;pollingFallback.value=false;publishConnected();wake.trySend(Unit)}}}
      override fun onMessage(webSocket:WebSocket,text:String){wake.trySend(Unit)}
      override fun onClosing(webSocket:WebSocket,code:Int,reason:String){webSocket.close(code,reason);ended.complete(Unit)}
      override fun onClosed(webSocket:WebSocket,code:Int,reason:String){ended.complete(Unit)}
      override fun onFailure(webSocket:WebSocket,t:Throwable,response:Response?){ended.complete(Unit)}
     })
     socket=ws
     try{ended.await()}finally{ws.cancel();if(socket===ws)socket=null}
    }catch(e:CancellationException){throw e}catch(_:Exception){}
    // HTTP is the source of connection health. A proxy can reject WebSocket
    // while authenticated reads and writes continue to work normally.
    pollingFallback.value=true;publishConnected();wake.trySend(Unit)
    delay(SyncPolicy.retryDelay(failures++)+kotlin.random.Random.nextLong(250))
   }
  }
 }
 private fun stopSession(){session?.cancel();session=null;socket?.cancel();socket=null}
 fun close(){stopSession();foregroundScope=null}
 suspend fun forget(){stopSession();sessionGate.reset{credentials.clear();readState.clear();library.clear();searchState.value=ConversationSearchState();redirects.value=emptyMap();draftPreview.clear();withContext(Dispatchers.IO){db.clearAllTables();imageMemory.evictAll();imageCache.deleteRecursively()};selected=null;captureWarning.value=null;capturePrefs.edit().clear().apply();httpHealthy=false;connectionIssue.value=null;connection.value="未连接"}}
}
