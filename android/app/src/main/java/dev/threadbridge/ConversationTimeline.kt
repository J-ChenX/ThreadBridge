package dev.threadbridge

import java.time.ZoneId

internal data class MessageRun(val role:String,val messages:List<MessageRow>) {
 val key get()="run:${messages.first().id}"
 val timestamp get()=messageTimeRange(messages.map{if(it.timestamp>0)it.timestamp else it.ordinal})
}
// Native commentary has a distinct stable ID; finals are notify: records. Unknown IDs stay visible.
internal fun isProcess(message:MessageRow)=message.role=="assistant"&&message.id.startsWith("native:")
internal fun messageRuns(newestFirst:List<MessageRow>):List<MessageRun> {
 val groups=mutableListOf<MutableList<MessageRow>>()
 for(message in newestFirst.asReversed()) {
  val last=groups.lastOrNull()
  if(last?.first()?.role==message.role)last.add(message)
  else groups+=mutableListOf(message)
 }
 return groups.asReversed().map{MessageRun(it.first().role,it.toList())}
}
internal fun messageTimeRange(times:List<Long>,zone:ZoneId=ZoneId.systemDefault()):String {
 val known=times.filter{it>0};if(known.isEmpty())return "时间未知"
 val first=known.minOrNull()!!;val last=known.maxOrNull()!!
 if(times.size==1)return messageTimestamp(first,zone)
 return "${messageTimestamp(first,zone)} — ${messageTimestamp(last,zone)}"
}
