package dev.threadbridge

/** A late transport acknowledgement cannot undo authoritative execution evidence. */
object SyncPolicy {
 private val terminal=setOf("codex_accepted","history_loaded","rejected","expired","cancelled")
 fun acceptUpdate(current:String,incoming:String):Boolean {
  if(current in terminal)return incoming==current
  if(current=="unknown")return incoming in terminal || incoming=="unknown"
  val order=mapOf("submitting" to 0,"accepted" to 1,"dispatching" to 2,"upstream_queued" to 3)
  return incoming !in order || (order[incoming]?:0)>=(order[current]?:0)
 }
 fun retryDelay(attempt:Int):Long=1000L shl attempt.coerceIn(0,5)
}

/** Serializes complete network-to-database operations with credential replacement.
 * Waiting work retains its original generation and cannot write into a new pairing. */
class SyncSessionGate {
 private val mutex=kotlinx.coroutines.sync.Mutex()
 private val generation=java.util.concurrent.atomic.AtomicLong()
 fun current():Long=generation.get()
 /** Called only while run holds the mutex, invalidating queued old cache writes. */
 fun invalidateWithinOperation(){generation.incrementAndGet()}
 suspend fun <T> run(expected:Long=current(),block:suspend()->T):T {
  mutex.lock()
  try { if(expected!=current())throw kotlinx.coroutines.CancellationException("Connection changed");return block() }
  finally { mutex.unlock() }
 }
 suspend fun <T> reset(block:suspend()->T):T {
  mutex.lock()
  try { return block() }
  finally { generation.incrementAndGet();mutex.unlock() }
 }
}
