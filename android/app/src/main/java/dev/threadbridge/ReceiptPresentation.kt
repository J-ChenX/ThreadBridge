package dev.threadbridge

/** Transport queue acknowledgement is never proof that execution began. */
object ReceiptPresentation {
 enum class Evidence { NONE, QUEUED, EXECUTOR_ACCEPTED, FINAL_CAPTURED }
 fun evidence(status:String,captureMode:Boolean):Evidence=when(status){
  "upstream_queued"->Evidence.QUEUED
  "codex_accepted"->if(captureMode)Evidence.FINAL_CAPTURED else Evidence.EXECUTOR_ACCEPTED
  else->Evidence.NONE
 }
 fun label(status:String,captureMode:Boolean)=when(status){
  "submitting"->"提交待确认";"accepted"->"中转已接收";"dispatching"->"正在分发"
  "upstream_queued"->"已入桌面队列，尚未确认开始执行"
  "codex_accepted"->if(evidence(status,captureMode)==Evidence.FINAL_CAPTURED)"最终回复已同步" else "任务已接受"
  "unknown"->"结果未知，请勿重复发送";"expired"->"请求已过期";"cancelled"->"已取消";"history_loaded"->"历史已更新";else->"已拒绝"
 }
 fun errorLabel(code:String)=when(code){
  "user_action_required"->"后台未授权并已停止。请在电脑核对审批或输入后继续原任务，不要重复发送。"
  "desktop_dynamic_tool_unavailable"->"原任务需要桌面工具，后台无法调用。请在电脑继续原任务，不要重复发送。"
  "interrupted_resume_no_retry","upstream_timeout","upstream_disconnected"->"后台执行结果未确认，已停止自动提交；请核对电脑原任务。"
  else->code
 }
 fun hint(status:String?,captureMode:Boolean)=if(status=="upstream_queued"&&captureMode)"桌面任务未激活时可能等待；请勿重复发送。" else if(captureMode)"仅显示启用后的最终回复，不包含原任务完整历史。" else null
}
