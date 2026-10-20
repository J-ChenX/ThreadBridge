package dev.threadbridge

/** The selected desktop owns display grouping; hostId always identifies execution. */
internal data class NewConversationProject(val ownerId:String,val hostId:String,val path:String,val name:String) {
 val key:String get()="$hostId\u0000$path"
}
internal fun newConversationProjects(ownerId:String,catalog:List<ProjectRow>,remote:List<NewConversationProject>,threads:List<ThreadRow>):List<NewConversationProject> {
 val local=catalog.filter{it.hostId==ownerId}.map{NewConversationProject(ownerId,ownerId,it.path,it.name)}
 val cached=threads.filter{it.groupHostId==ownerId&&it.projectKnown}.map{NewConversationProject(ownerId,it.hostId,it.project,it.projectName)}
 return (remote.filter{it.ownerId==ownerId}+local+cached+NewConversationProject(ownerId,ownerId,"","")).distinctBy{it.key}.sortedWith(compareBy({it.hostId!=ownerId},{it.hostId},{if(it.path.isEmpty())"\uffff"else it.path}))
}
