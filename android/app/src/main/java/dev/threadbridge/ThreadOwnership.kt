package dev.threadbridge

/** Display grouping never changes hostId, which remains the execution destination. */
internal val ThreadRow.groupHostId:String get()=displayHostId.ifBlank{hostId}
