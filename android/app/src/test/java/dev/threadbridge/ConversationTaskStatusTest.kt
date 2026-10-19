package dev.threadbridge
import org.junit.Assert.*
import org.junit.Test
class ConversationTaskStatusTest {
 private fun thread(task:String,online:Boolean=true)=ThreadRow("id","title","host","queue_ready","final-turn",0,true,online,null,taskStatus=task)
 @Test fun executionStateIsIndependentOfCaptureCapabilityAndCompletedRevision(){assertTrue(taskIsRunning(thread("running")));assertTrue(taskIsRunning(thread("inProgress")));assertFalse(taskIsRunning(thread("completed")));assertFalse(taskIsRunning(thread("unknown")));assertFalse(taskIsRunning(null))}
 @Test fun offlineDoesNotClaimCachedExecutionIsLive(){assertFalse(taskIsRunning(thread("running",false)));assertFalse(taskIsRunning(thread("interrupted")));assertFalse(taskIsRunning(thread("failed")))}
}
