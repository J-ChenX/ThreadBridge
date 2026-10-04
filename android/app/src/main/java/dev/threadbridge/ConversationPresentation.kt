package dev.threadbridge

object ConversationPresentation {
 fun showPending(status:String):Boolean = status !in setOf("codex_accepted","history_loaded")
 fun showPendingText(status:String,text:String,imported:Boolean):Boolean = showPending(status)&&text.isNotBlank()&&!imported
}
