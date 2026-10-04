package dev.threadbridge
import org.junit.Assert.*
import org.junit.Test
class ReceiptPresentationTest {
 @Test fun approvalAndMissingToolsAreActionableWithoutRetry(){assertTrue(ReceiptPresentation.errorLabel("user_action_required").contains("未授权"));assertTrue(ReceiptPresentation.errorLabel("desktop_dynamic_tool_unavailable").contains("不要重复发送"));assertTrue(ReceiptPresentation.errorLabel("upstream_timeout").contains("停止自动提交"))}
 @Test fun queuedCannotProveExecutionOrCompletion(){for(capture in listOf(false,true)){assertEquals(ReceiptPresentation.Evidence.QUEUED,ReceiptPresentation.evidence("upstream_queued",capture));assertEquals(ReceiptPresentation.Evidence.NONE,ReceiptPresentation.evidence("accepted",capture));assertEquals(ReceiptPresentation.Evidence.NONE,ReceiptPresentation.evidence("dispatching",capture))}}
 @Test fun completionLabelRequiresCaptureReceipt(){assertEquals(ReceiptPresentation.Evidence.FINAL_CAPTURED,ReceiptPresentation.evidence("codex_accepted",true));assertEquals(ReceiptPresentation.Evidence.EXECUTOR_ACCEPTED,ReceiptPresentation.evidence("codex_accepted",false));assertNotEquals(ReceiptPresentation.label("codex_accepted",true),ReceiptPresentation.label("codex_accepted",false));assertNull(ReceiptPresentation.hint("upstream_queued",false));assertNotNull(ReceiptPresentation.hint("upstream_queued",true))}
}
