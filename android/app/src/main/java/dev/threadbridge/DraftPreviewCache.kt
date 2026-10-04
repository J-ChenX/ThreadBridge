package dev.threadbridge

/** Latest queued draft, before its serialized persistent write can finish. */
internal class DraftPreviewCache {
 private data class Entry(val generation:Long,val text:String)
 private val pending=java.util.concurrent.ConcurrentHashMap<String,Entry>()
 fun put(id:String,generation:Long,text:String){pending[id]=Entry(generation,text)}
 fun get(id:String,generation:Long)=pending[id]?.takeIf{it.generation==generation}?.text
 fun committed(id:String,generation:Long,text:String){pending.remove(id,Entry(generation,text))}
 fun clear(){pending.clear()}
}
