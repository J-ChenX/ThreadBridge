package dev.threadbridge

/** A release-scoped move to the same paired Hub, never an arbitrary redirect. */
object PublicTestConnection {
 fun shouldMigrate(current:String,token:String,previous:String,target:String,host:String):Boolean =
  token.isNotEmpty() && previous.split(',').any{it.isNotBlank()&&it==current} &&
   target.startsWith("https://") && target!=current && host.isNotEmpty()
 fun usePolling(current:String,target:String):Boolean = target.startsWith("https://") && current==target
 fun matchesHost(actual:List<String>,expected:String):Boolean =
  expected.isNotEmpty() && expected in actual
}
