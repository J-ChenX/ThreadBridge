package dev.threadbridge
import org.junit.Assert.*
import org.junit.Test
class SyncPolicyTest {
 @Test fun queuedIsNotFinalAndCannotRegress(){assertTrue(SyncPolicy.acceptUpdate("dispatching","upstream_queued"));assertTrue(SyncPolicy.acceptUpdate("upstream_queued","codex_accepted"));assertFalse(SyncPolicy.acceptUpdate("upstream_queued","accepted"))}
 @Test fun lateAcknowledgementCannotRegressExecutionReceipt(){assertFalse(SyncPolicy.acceptUpdate("codex_accepted","accepted"));assertFalse(SyncPolicy.acceptUpdate("dispatching","accepted"))}
 @Test fun unknownOnlyResolvesWithAuthoritativeResult(){assertFalse(SyncPolicy.acceptUpdate("unknown","dispatching"));assertTrue(SyncPolicy.acceptUpdate("unknown","codex_accepted"));assertTrue(SyncPolicy.acceptUpdate("unknown","rejected"))}
 @Test fun reconnectBackoffIsBounded(){assertEquals(1000L,SyncPolicy.retryDelay(0));assertEquals(32000L,SyncPolicy.retryDelay(100))}
 @Test fun normalProgressAndHistoryCompletionAreAccepted(){assertTrue(SyncPolicy.acceptUpdate("submitting","accepted"));assertTrue(SyncPolicy.acceptUpdate("accepted","dispatching"));assertTrue(SyncPolicy.acceptUpdate("dispatching","history_loaded"))}
}
