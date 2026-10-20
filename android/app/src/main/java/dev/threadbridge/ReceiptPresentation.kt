package dev.threadbridge

/** Transport queue acknowledgement is never proof that execution began. */
object ReceiptPresentation {
 enum class Evidence { NONE, QUEUED, EXECUTOR_ACCEPTED, FINAL_CAPTURED }
 fun evidence(status:String,captureMode:Boolean):Evidence=when(status){
  "upstream_queued"->Evidence.QUEUED
  "steer_accepted"->Evidence.EXECUTOR_ACCEPTED
  "codex_accepted"->if(captureMode)Evidence.FINAL_CAPTURED else Evidence.EXECUTOR_ACCEPTED
  else->Evidence.NONE
 }
 fun label(status:String,captureMode:Boolean)=when(status){
  "submitting"->"提交待确认";"accepted"->"中转已接收";"dispatching"->"正在分发"
  "steer_accepted"->"引导已提交到当前轮次"
  "upstream_queued"->"已入桌面队列，尚未确认开始执行"
  "codex_accepted"->if(evidence(status,captureMode)==Evidence.FINAL_CAPTURED)"最终回复已同步" else "任务已接受"
  "unknown"->"结果未知，请勿重复发送";"expired"->"请求已过期";"cancelled"->"已取消";"history_loaded"->"历史已更新";else->"已拒绝"
 }
 fun errorLabel(code:String)=when(code){
  "steer_in_progress"->"正在将这条消息作为引导发送…"
  "steer_not_ready"->"当前轮次已变化，请同步状态后重试。"
  "steer_queue_missing"->"这条消息已不在电脑队列，请同步核对处理结果。"
  "steer_queue_changed"->"消息内容已在电脑修改，暂不转为引导。"
  "steer_unavailable"->"电脑引导服务暂不可用，消息仍在原队列。请打开电脑 Codex 后重试。"
  "steer_turn_changed"->"当前轮次已变化，这条消息未作为引导发送，请核对后重新发送。"
  "steer_result_unknown_no_retry"->"引导结果待确认，请勿重复发送；同步后核对电脑消息。"
  "question_no_longer_pending"->"这个问题已在其他端处理，请同步查看。"
  "invalid_question_reply"->"回答内容无效，请检查后重试。"
  "creation_executor_unavailable"->"电脑创建服务暂不可用，消息未发送。请先打开对应电脑的 Codex，再重试。"
  "creation_unconfirmed_no_retry"->"未能确认新对话是否创建，请在电脑核对结果，勿重复发送。"
  "user_action_required"->"后台未授权并已停止。请在电脑核对审批或输入后继续原任务，不要重复发送。"
  "desktop_dynamic_tool_unavailable"->"原任务需要桌面工具，后台无法调用。请在电脑继续原任务，不要重复发送。"
  "interrupted_resume_no_retry","upstream_timeout","upstream_disconnected"->"后台执行结果未确认，已停止自动提交；请核对电脑原任务。"
  else->code
 }
 fun hint(status:String?,captureMode:Boolean)=if(status=="upstream_queued"&&captureMode)"桌面任务未激活时可能等待；请勿重复发送。" else if(captureMode)"仅显示启用后的最终回复，不包含原任务完整历史。" else null
}
/** The immutable send ledger owns submitted text, including uncertain requests. */
internal fun composerSubmitted(status:String)=status in setOf("submitting","accepted","dispatching","upstream_queued","codex_accepted","unknown","steer_accepted")

internal fun canSteerPending(status:String,error:String?,running:Boolean)=status=="upstream_queued"&&error in setOf(null,"steer_unavailable","steer_not_ready")&&running
