package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test

class ConversationScopeTest {
 private fun row(id:String,host:String="a",project:String="/work",known:Boolean=true)=ThreadRow(id,id,host,"completed","r",1,true,true,null,project=project,projectKnown=known)
 @Test fun deviceReadIncludesOlderCollapsedConversationsWithoutCrossingDevices() {
  val rows=(1..10).map{row("$it")}+row("other","b")+row("other-project",project="/second")
  val scope=ConversationScope("a",title="Device A")
  val read=rows.filter(scope::contains).associate{it.id to conversationReadStamp(it)}
  assertEquals(setOf("other"),unreadConversations(rows,read))
  assertEquals(setOf("1","other"),unreadConversations(rows.map{if(it.id=="1")it.copy(messageRevision=2)else it},read))
 }
 @Test fun projectReadKeepsOtherDeviceAndUnknownProjectUnread() {
  val rows=listOf(row("selected"),row("second",project="/second"),row("other","b"),row("unknown",known=false))
  val scope=ConversationScope("a","/work",true,"Work")
  val read=rows.filter(scope::contains).associate{it.id to conversationReadStamp(it)}
  assertEquals(setOf("second","other","unknown"),unreadConversations(rows,read))
 }
 @Test fun emptyProjectIsScopedRatherThanAllProjects() {
  val scope=ConversationScope("a","",true,"其他")
  assertTrue(scope.contains(row("empty",project="")))
  assertFalse(scope.contains(row("work")))
 }
}
