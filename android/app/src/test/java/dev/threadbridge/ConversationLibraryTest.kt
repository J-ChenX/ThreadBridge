package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test
import java.time.ZoneId

class ConversationLibraryTest {
 private fun row(id:String,project:String="",time:Long=1)=ThreadRow(id,id,"host","completed","r",time,true,true,null,project=project,projectKnown=true)
 @Test fun projectGroupingUsesFullPathAndPinsThenActivity(){
  val groups=projectGroups(listOf(row("a","/first/name",30),row("b","/second/name",20),row("old","/first/name",1),row("none")),setOf("old","b"))
  assertEquals(3,groups.size);assertEquals("/first/name",groups[0].project);assertEquals(listOf("old","a"),groups[0].threads.map{it.id});assertEquals("其他",projectLabel(groups.last().project));assertEquals("repo",projectLabel("C:\\code\\repo\\"))
  val pins=projectGroups(listOf(row("p1","/p",1),row("p2","/p",2),row("new","/p",3)),setOf("p1","p2")).single()
  assertEquals(listOf("p2","p1","new"),pins.threads.map{it.id})
 }
 @Test fun previewIsRecentThreeUnionUnreadWithPersistentPinsAndBlankDrafts(){
  val rows=(1..8).map{row("$it",time=it.toLong())}+row("blank",time=0).copy(localOnly=true)
  val group=deviceGroups(listOf(HostRow("host","Host",true)),rows,setOf("1","8"),setOf("2")).single()
  assertEquals(listOf("8","7","6","2","1","blank"),group.preview.map{it.id})
 }
 @Test fun cleanupWaitsOneWeekAndProtectsDraftsRequestsFavoritesPinsAndCurrent(){
  val thread=row("x");val deadline=1000+CONVERSATION_RETENTION_MS
  assertFalse(canExpire(thread,deadline-1,emptySet(),null,false,null));assertTrue(canExpire(thread,deadline,emptySet(),null,false,null))
  assertFalse(canExpire(thread,deadline,setOf("x"),null,false,null));assertFalse(canExpire(thread,deadline,emptySet(),"draft",false,null));assertFalse(canExpire(thread,deadline,emptySet(),null,true,null));assertFalse(canExpire(thread,deadline,emptySet(),null,false,"x"))
 }
 @Test fun unavailableMetadataIsDistinctFromVerifiedOther(){
  val rows=listOf(row("known").copy(projectKnown=true),row("pending").copy(projectKnown=false))
  assertEquals(2,projectGroups(rows,emptySet()).size);assertEquals("其他",threadProjectLabel(rows[0]));assertEquals("项目待同步",threadProjectLabel(rows[1]));assertNotEquals(projectKey("host","",true),projectKey("host","",false))
 }
 @Test fun fullTimestampUsesDeviceZoneAndNoOrderAsSeconds(){
  assertEquals("2023-11-15 06:13:20",messageTimestamp(1700000000000,ZoneId.of("Asia/Shanghai")));assertEquals("时间未知",messageTimestamp(0))
 }
 @Test fun eachProjectKeepsItsOwnRecentThreeAndOlderUnread(){
  val rows=(1..5).map{row("a$it","/a",it.toLong()+100)}+(1..5).map{row("b$it","/b",it.toLong())}
  val unread=setOf("b1");val pinned=setOf("a1")
  val device=deviceGroups(listOf(HostRow("host","Host",true)),rows,unread,pinned).single()
  assertEquals(setOf("a5","a4","a3","a1","b5","b4","b3","b1"),device.preview.map{it.id}.toSet())
  val projects=projectGroups(device.threads,pinned)
  assertEquals(2,projects.size)
  assertEquals(listOf("a1","a5","a4","a3"),projectPreview(projects.first(),unread,pinned).map{it.id})
  assertEquals(listOf("b5","b4","b3","b1"),projectPreview(projects.last(),unread,pinned).map{it.id})
 }
 @Test fun fuzzyTitleSearchSupportsFragmentsCaseWhitespaceAndFullWidth(){
  assertTrue(titleMatches("优化手机端对话界面","手机界面"));assertTrue(titleMatches("Project Alpha","ＰＲＯ alpha"));assertTrue(titleMatches("其他对话","  "));assertFalse(titleMatches("优化手机界面","界手机"));assertTrue(titleMatches("100%_Done","%_done"));assertTrue(titleMatches("Équipe discussion","équipe"))
 }
}
