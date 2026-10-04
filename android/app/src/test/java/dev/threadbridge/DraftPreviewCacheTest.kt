package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test

class DraftPreviewCacheTest {
 @Test fun olderWriteCannotEraseNewerQueuedDraft() {
  val cache=DraftPreviewCache();cache.put("t",1,"first");cache.put("t",1,"latest")
  cache.committed("t",1,"first");assertEquals("latest",cache.get("t",1))
  cache.committed("t",1,"latest");assertNull(cache.get("t",1))
 }
 @Test fun resetGenerationCannotRestoreOrEraseAnOldSessionsDraft() {
  val cache=DraftPreviewCache();cache.put("t",1,"old");assertNull(cache.get("t",2))
  cache.put("t",2,"new");cache.committed("t",1,"old");assertEquals("new",cache.get("t",2))
  cache.clear();assertNull(cache.get("t",2))
 }
 @Test fun emptyDraftIsAQueuedClearAndThreadsStayIndependent() {
  val cache=DraftPreviewCache();cache.put("a",1,"");cache.put("b",1,"text")
  assertEquals("",cache.get("a",1));assertEquals("text",cache.get("b",1))
 }
}
