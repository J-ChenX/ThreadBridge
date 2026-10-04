package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test

class ConversationPresentationTest {
 @Test fun completedInputIsNotRepeatedAfterAssistantReply() {
  assertFalse(ConversationPresentation.showPending("codex_accepted"))
  assertFalse(ConversationPresentation.showPending("history_loaded"))
 }
 @Test fun importedInputIsNotRepeatedWhileReceiptIsPending() {
  assertFalse(ConversationPresentation.showPendingText("upstream_queued","phone text",true))
  assertTrue(ConversationPresentation.showPendingText("unknown","phone text",false))
 }
 @Test fun uncertainAndFailedSendsKeepTheirTextAndControls() {
  for(status in listOf("submitting","accepted","dispatching","upstream_queued","unknown","rejected","expired","cancelled"))
   assertTrue(status,ConversationPresentation.showPending(status))
 }
}
