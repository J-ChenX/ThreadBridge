package dev.threadbridge
import org.junit.Assert.*
import org.junit.Test
import java.time.ZoneId
class ConversationTimelineTest {
 private fun m(id:String,role:String,time:Long)=MessageRow("t",id,"turn",role,"body","v",time,4,time)
 @Test fun consecutiveRolesShareOneRangeAndUnknownAssistantRemainsVisible(){
  val rows=listOf(m("notify:f","assistant",3000),m("native:c","assistant",2000),m("u2","user",1500),m("u1","user",1000))
  val runs=messageRuns(rows);assertEquals(2,runs.size);assertEquals(listOf("u1","u2"),runs[1].messages.map{it.id});assertEquals(2,runs[0].messages.size)
  assertTrue(isProcess(rows[1]));assertFalse(isProcess(rows[0]));assertFalse(isProcess(m("unknown","assistant",0)))
  val zone=ZoneId.of("UTC");assertEquals("1970-01-01 00:00:01 — 1970-01-01 00:00:03",messageTimeRange(listOf(1000,3000),zone));assertEquals("1970-01-01 00:00:01",messageTimeRange(listOf(1000),zone))
 }
 @Test fun largeConsecutiveRunPreservesOrderWithoutSplittingTimestamp(){
  val rows=(10000 downTo 1).map{MessageRow("t","$it","turn","assistant","body","v",it.toLong(),4,it.toLong())}
  val runs=messageRuns(rows);assertEquals(1,runs.size);assertEquals(10000,runs.single().messages.size);assertEquals("1",runs.single().messages.first().id);assertEquals("10000",runs.single().messages.last().id)
 }
}
