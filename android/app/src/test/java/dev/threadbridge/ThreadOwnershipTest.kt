package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test

class ThreadOwnershipTest {
 @Test fun remoteTaskGroupsAndBulkSelectionFollowDesktopWhileExecutionRemainsRemote(){
  val remote=ThreadRow("ssh","task","linux","completed","r",42,false,false,null,project="/work",projectKnown=true,displayHostId="windows")
  val local=remote.copy(id="local",hostId="windows",displayHostId="",project="C:/work")
  val groups=deviceGroups(listOf(HostRow("linux","linux",true),HostRow("windows","windows",true)),listOf(remote,local),emptySet())
  assertEquals(setOf("ssh","local"),groups.single{it.host.id=="windows"}.threads.map{it.id}.toSet())
  assertTrue(groups.single{it.host.id=="linux"}.threads.isEmpty())
  assertTrue(ConversationScope("windows",title="Windows").contains(remote))
  assertFalse(ConversationScope("linux",title="Linux").contains(remote))
  assertEquals("linux",remote.hostId)
  assertFalse(remote.canSend)
  assertEquals(conversationReadStamp(remote.copy(displayHostId="")),conversationReadStamp(remote))
 }
}
