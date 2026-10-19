package dev.threadbridge

/** Null project selects the whole device; known/unknown project groups stay separate. */
internal data class ConversationScope(val host:String,val project:String?=null,val known:Boolean=true,val title:String) {
 fun contains(thread:ThreadRow)=thread.groupHostId==host&&(project==null||(thread.project==project&&thread.projectKnown==known))
}
