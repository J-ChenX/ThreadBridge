package dev.threadbridge

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.unit.Velocity
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test

class ConversationEdgePullTest {
 @Test fun normalReadingAndUnavailableEdgesDoNotTriggerActions() {
  assertFalse(conversationPullAllowed(-50f,false,false,false,false,true))
  assertFalse(conversationPullAllowed(50f,false,false,false,false,true))
  assertTrue(conversationPullAllowed(-50f,true,false,false,false,true))
  assertTrue(conversationPullAllowed(50f,false,true,false,false,true))
  assertFalse(conversationPullAllowed(-50f,true,false,true,false,true))
  assertFalse(conversationPullAllowed(50f,false,true,false,true,true))
  assertFalse(conversationPullAllowed(50f,false,true,false,false,false))
 }
 @Test fun shortPullAndFlingAloneNeverRefresh()=runBlocking {
  val pull=ConversationEdgePull(100f);var refreshes=0
  pull.allowed={it<0f};pull.refresh={refreshes++}
  pull.onPostScroll(Offset.Zero,Offset(0f,-99f),NestedScrollSource.UserInput)
  pull.onPreFling(Velocity(0f,-2000f))
  assertEquals(0,refreshes);assertEquals(0f,pull.distance,0f)
  pull.onPostScroll(Offset.Zero,Offset(0f,-300f),NestedScrollSource.SideEffect)
  pull.onPreFling(Velocity(0f,-2000f))
  assertEquals(0,refreshes)
 }
 @Test fun releaseTriggersEachEdgeOnceAndRespectsReversal()=runBlocking {
  val pull=ConversationEdgePull(100f);var refreshes=0;var pages=0
  pull.allowed={true};pull.refresh={refreshes++};pull.older={pages++}
  pull.onPostScroll(Offset.Zero,Offset(0f,-120f),NestedScrollSource.UserInput)
  pull.onPreScroll(Offset(0f,30f),NestedScrollSource.UserInput)
  pull.onPreFling(Velocity.Zero)
  assertEquals(0,refreshes)
  pull.onPostScroll(Offset.Zero,Offset(0f,-120f),NestedScrollSource.UserInput)
  pull.onPreFling(Velocity.Zero);pull.onPreFling(Velocity.Zero)
  assertEquals(1,refreshes)
  pull.onPostScroll(Offset.Zero,Offset(0f,120f),NestedScrollSource.UserInput)
  pull.onPreFling(Velocity.Zero)
  assertEquals(1,pages)
 }
 @Test fun operationBecomingBusyBeforeReleasePreventsDuplicate()=runBlocking {
  val pull=ConversationEdgePull(100f);var count=0
  pull.allowed={true};pull.refresh={count++}
  pull.onPostScroll(Offset.Zero,Offset(0f,-120f),NestedScrollSource.UserInput)
  pull.allowed={false};pull.onPreFling(Velocity.Zero)
  assertEquals(0,count);assertEquals(0f,pull.distance,0f)
 }
}
