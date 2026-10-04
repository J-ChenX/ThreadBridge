package dev.threadbridge

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow

/** Read markers are local to this phone and collection; drafts/pairing are untouched. */
internal class ConversationReadState(context:Context) {
 private val prefs=context.getSharedPreferences("conversation_reads",Context.MODE_PRIVATE)
 val unread=MutableStateFlow<Set<String>>(emptySet())
 private var rows=emptyList<ThreadRow>()
 private var stamps=emptyMap<String,String>()
 private var initialized=prefs.contains("collection")
 fun restore(threads:List<ThreadRow>) {
  if(!initialized)return
  rows=threads;stamps=threads.associate{it.id to (prefs.getString(it.id,null)?:"")}
  unread.value=unreadConversations(threads,stamps)
 }
 fun observe(collection:String,threads:List<ThreadRow>) {
  rows=threads
  // The first synchronized snapshot after upgrade/pair/reset is the baseline.
  // Old imported history must not suddenly turn every device blue.
  if(!initialized||prefs.getString("collection",null)!=collection) {
   val editor=prefs.edit().clear().putString("collection",collection)
   threads.forEach{editor.putString(it.id,conversationReadStamp(it))};editor.apply();initialized=true
  }
  restore(threads)
  val live=threads.map{it.id}.toSet()
  val editor=prefs.edit();prefs.all.keys.filter{it!="collection"&&it !in live}.forEach{editor.remove(it)};editor.apply()
 }
 fun markRead(thread:ThreadRow) {
  if(!initialized)return
  val stamp=conversationReadStamp(thread)
  prefs.edit().putString(thread.id,stamp).apply();stamps=stamps+(thread.id to stamp)
  unread.value=unreadConversations(rows,stamps)
 }
 fun clear(){prefs.edit().clear().apply();rows=emptyList();stamps=emptyMap();initialized=false;unread.value=emptySet()}
}
// Keep legacy markers valid until an actual message mutation occurs.
internal fun conversationReadStamp(thread:ThreadRow)="${thread.updated}:${thread.revision}"+(if(thread.messageRevision==0L)""else":${thread.messageRevision}")
internal fun conversationActivity(thread:ThreadRow)=maxOf(thread.updated*1000,thread.messageActivityAt)
internal fun unreadConversations(threads:List<ThreadRow>,read:Map<String,String>)=threads.filter{read[it.id]!=conversationReadStamp(it)}.map{it.id}.toSet()
internal data class DeviceGroup(val host:HostRow,val threads:List<ThreadRow>,val preview:List<ThreadRow>,val unread:Boolean)
internal fun deviceGroups(hosts:List<HostRow>,threads:List<ThreadRow>,unread:Set<String>):List<DeviceGroup> = hosts.map{host->
 val group=threads.filter{it.hostId==host.id}.sortedWith(compareByDescending<ThreadRow>{conversationActivity(it)}.thenBy{it.id})
 val recent=group.take(5).map{it.id}.toSet()
 DeviceGroup(host,group,group.filter{it.id in recent||it.id in unread},group.any{it.id in unread})
}.sortedWith(compareByDescending<DeviceGroup>{it.threads.firstOrNull()?.let(::conversationActivity)?:Long.MIN_VALUE}.thenBy{it.host.name}.thenBy{it.host.id})
