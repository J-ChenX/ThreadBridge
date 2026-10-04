package dev.threadbridge
import kotlinx.coroutines.*
import org.junit.Assert.*
import org.junit.Test

class SyncSessionGateTest {
 @Test fun oldHttpResponseCannotRefillAfterForget()=runBlocking {
  val gate=SyncSessionGate();val started=CompletableDeferred<Unit>();val response=CompletableDeferred<Unit>();val cache=mutableListOf<String>()
  val oldRequest=launch {gate.run {started.complete(Unit);response.await();cache.add("old response")}}
  started.await()
  val clear=launch(start=CoroutineStart.UNDISPATCHED){gate.reset{cache.clear()}}
  val stale=launch(start=CoroutineStart.UNDISPATCHED){gate.run{cache.add("queued old response")}}
  response.complete(Unit);joinAll(oldRequest,clear,stale)
  assertTrue(cache.isEmpty());assertTrue(stale.isCancelled)
 }
 @Test fun queuedDraftBodyAndReceiptCannotCrossPairing()=runBlocking {
  val gate=SyncSessionGate();val generation=gate.current();val cache=mutableListOf("old")
  gate.reset{cache.clear();cache.add("new account")}
  for(value in listOf("draft","body chunk","command receipt")){
   try{gate.run(generation){cache.add(value)};fail("stale writer was accepted")}catch(_:CancellationException){}
  }
  gate.run{cache.add("new response")}
  assertEquals(listOf("new account","new response"),cache)
 }
 @Test fun collectionResetRejectsQueuedDraftAndKeepsCurrentSync()=runBlocking {
  val gate=SyncSessionGate();val previous=gate.current();val cache=mutableListOf("old")
  gate.run {gate.invalidateWithinOperation();cache.clear();cache.add("new collection")}
  try{gate.run(previous){cache.add("old draft")};fail("old draft crossed reset")}catch(_:CancellationException){}
  gate.run{cache.add("new response")}
  assertEquals(listOf("new collection","new response"),cache)
 }
}
