package dev.threadbridge

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

internal data class ConversationMarks(val pinned:Set<String> = emptySet(),val favorites:Set<String> = emptySet())
internal class ConversationLibrary(context:Context) {
 private val prefs=context.getSharedPreferences("conversation_library",Context.MODE_PRIVATE)
 val marks=MutableStateFlow(ConversationMarks(prefs.getStringSet("pinned",emptySet())!!.toSet(),prefs.getStringSet("favorites",emptySet())!!.toSet()))
 fun pin(id:String){val value=marks.value;save(value.copy(pinned=toggle(value.pinned,id)))}
 fun favorite(id:String){val value=marks.value;save(value.copy(favorites=toggle(value.favorites,id)))}
 private fun toggle(ids:Set<String>,id:String)=if(id in ids)ids-id else ids+id
 private fun save(value:ConversationMarks){prefs.edit().putStringSet("pinned",value.pinned).putStringSet("favorites",value.favorites).apply();marks.value=value}
 fun remove(id:String){save(marks.value.copy(pinned=marks.value.pinned-id,favorites=marks.value.favorites-id));prefs.edit().remove("expired:$id").apply()}
 fun move(from:String,to:String){val m=marks.value;save(ConversationMarks(if(from in m.pinned)m.pinned-from+to else m.pinned,if(from in m.favorites)m.favorites-from+to else m.favorites))}
 fun restore(id:String){prefs.edit().remove("expired:$id").apply()}
 fun expired(thread:ThreadRow):Boolean {val old=prefs.getString("expired:${thread.id}",null);return old=="${conversationActivity(thread)}|${conversationReadStamp(thread)}"}
 fun expire(thread:ThreadRow){prefs.edit().putString("expired:${thread.id}","${conversationActivity(thread)}|${conversationReadStamp(thread)}").apply()}
 fun reconcile(live:Set<String>){
  val value=marks.value;save(value.copy(pinned=value.pinned.intersect(live),favorites=value.favorites.intersect(live)))
  val editor=prefs.edit();prefs.all.keys.filter{it.startsWith("expired:")&&it.removePrefix("expired:") !in live}.forEach{editor.remove(it)};editor.apply()
 }
 fun cleanupDue(now:Long)=now-prefs.getLong("cleanup_at",0)>=6*60*60*1000L
 fun cleaned(now:Long){prefs.edit().putLong("cleanup_at",now).apply()}
 fun clear(){prefs.edit().clear().apply();marks.value=ConversationMarks()}
}
internal const val CONVERSATION_RETENTION_MS=7*24*60*60*1000L
internal fun canExpire(thread:ThreadRow,now:Long,protected:Set<String>,draft:String?,pending:Boolean,selected:String?):Boolean =
 thread.id!=selected&&thread.id !in protected&&draft.isNullOrBlank()&&!pending&&conversationActivity(thread)>0&&now-conversationActivity(thread)>=CONVERSATION_RETENTION_MS
internal fun projectLabel(project:String)=project.trimEnd('/','\\').substringAfterLast('/').substringAfterLast('\\').ifBlank{"其他"}
internal fun projectKey(host:String,project:String,known:Boolean=true)="project:$host:$project:$known"
internal data class ProjectGroup(val project:String,val threads:List<ThreadRow>,val known:Boolean=true)
internal fun projectGroups(threads:List<ThreadRow>,pinned:Set<String>):List<ProjectGroup> = threads.groupBy{it.project to it.projectKnown}.map{(identity,rows)->
 val (project,known)=identity
 ProjectGroup(project,rows.sortedWith(compareByDescending<ThreadRow>{it.id in pinned}.thenByDescending{conversationActivity(it)}.thenBy{it.id}),known)
}.sortedWith(compareByDescending<ProjectGroup>{it.threads.any{t->t.id in pinned}}.thenByDescending{it.threads.maxOfOrNull(::conversationActivity)?:0L}.thenBy{it.project})
internal fun messageTimestamp(timestamp:Long,zone:ZoneId=ZoneId.systemDefault()):String = if(timestamp<=0)"时间未知" else DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss",Locale.ROOT).withZone(zone).format(Instant.ofEpochMilli(timestamp))

internal fun threadProjectLabel(thread:ThreadRow)=if(!thread.projectKnown)"项目待同步"else thread.projectName.ifBlank{projectLabel(thread.project)}

internal fun projectPreview(project:ProjectGroup,unread:Set<String>,pinned:Set<String>):List<ThreadRow>{
 val recent=project.threads.sortedWith(compareByDescending<ThreadRow>{conversationActivity(it)}.thenBy{it.id}).take(3).map{it.id}.toSet()
 return project.threads.filter{it.id in recent||it.id in unread||it.id in pinned||it.localOnly}
}
