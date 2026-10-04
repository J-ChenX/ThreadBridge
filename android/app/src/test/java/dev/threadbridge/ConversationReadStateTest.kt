package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test

class ConversationReadStateTest {
 private fun row(id:String,host:String="a",updated:Long=1,revision:String="r")=ThreadRow(id,id,host,"completed",revision,updated,true,true,null)
 @Test fun readMarkerSurvivesMetadataChangesButDetectsNewUpdates() {
  val original=row("x");val read=mapOf("x" to conversationReadStamp(original))
  assertTrue(unreadConversations(listOf(original.copy(title="renamed",online=false,status="active")),read).isEmpty())
  assertEquals(setOf("x"),unreadConversations(listOf(original.copy(updated=2)),read))
  assertEquals(setOf("x"),unreadConversations(listOf(original.copy(revision="next")),read))
  assertEquals(setOf("new"),unreadConversations(listOf(original,row("new")),read))
 }
 @Test fun previewHasFiveRecentAndEveryOlderUnread() {
  val threads=(1..10).map{row("$it",updated=it.toLong())}
  val groups=deviceGroups(listOf(HostRow("a","A",false)),threads,setOf("1","2"))
  assertEquals(listOf("10","9","8","7","6","2","1"),groups.single().preview.map{it.id})
 }
 @Test fun liveMessagesWithUnchangedCompletedTurnStillBecomeUnread() {
  val original=row("x").copy(messageRevision=7,messageActivityAt=1100)
  val read=mapOf("x" to conversationReadStamp(original))
  val updated=original.copy(messageRevision=8,messageActivityAt=1100)
  assertEquals(original.updated,updated.updated)
  assertEquals(original.revision,updated.revision)
  assertEquals(setOf("x"),unreadConversations(listOf(updated),read))
  assertTrue(unreadConversations(listOf(updated),mapOf("x" to conversationReadStamp(updated))).isEmpty())
 }
 @Test fun millisecondActivityReordersDevicesAndLegacyMarkersRemainValid() {
  val hosts=listOf(HostRow("a","A",true),HostRow("b","B",true))
  val earlier=row("old","a",1).copy(messageActivityAt=1100)
  val later=row("new","b",1).copy(messageActivityAt=1200)
  assertEquals(listOf("b","a"),deviceGroups(hosts,listOf(earlier,later),emptySet()).map{it.host.id})
  assertEquals("1:r",conversationReadStamp(row("legacy")))
  assertTrue(unreadConversations(listOf(row("legacy")),mapOf("legacy" to "1:r")).isEmpty())
 }
 @Test fun moreThanFiveUnreadAreAllVisible() {
  val threads=(1..12).map{row("$it",updated=it.toLong())}
  val unread=(1..8).map{it.toString()}.toSet()
  val group=deviceGroups(listOf(HostRow("a","A",true)),threads,unread).single()
  assertTrue(group.preview.map{it.id}.containsAll(unread))
  assertTrue(group.unread)
 }
 @Test fun newestDeviceMovesFirstEvenWhenPreviouslyBelowAndStaysAfterRead() {
  val hosts=listOf(HostRow("a","A",true),HostRow("b","B",true),HostRow("c","C",false))
  val threads=listOf(row("old","a",1),row("new","b",20))
  assertEquals(listOf("b","a","c"),deviceGroups(hosts,threads,setOf("new")).map{it.host.id})
  val readGroups=deviceGroups(hosts,threads,emptySet())
  assertEquals(listOf("b","a","c"),readGroups.map{it.host.id})
  assertFalse(readGroups.any{it.unread})
 }
 @Test fun emptyAndSmallDevicesDoNotInventPreviewRows() {
  val group=deviceGroups(listOf(HostRow("a","A",true)),listOf(row("one")),emptySet()).single()
  assertEquals(1,group.preview.size)
  assertTrue(deviceGroups(listOf(HostRow("a","A",false)),emptyList(),emptySet()).single().preview.isEmpty())
 }
}
