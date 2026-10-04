package dev.threadbridge

object CapturePresentation {
 fun summary(detail:String)=when {
  detail.contains("用户消息尚未同步")->"用户消息尚未同步"
  detail.contains("尚未确认保存")||detail.contains("未保存正文")->"部分回复尚未确认保存"
  else->"同步状态尚未确认"
 }
 fun reason(code:String)=when(code){
  "user_input_capture_failed"->"用户消息尚未同步，助手回复已保存"
  "disk_space_low"->"电脑磁盘余量不足"
  "storage_budget_exceeded"->"已达到配置的存储预算"
  "reply_too_large"->"正文超过单条保存限制，未截断保存"
  "conflicting_reply"->"同一回复 ID 出现不同正文"
  "capture_not_confirmed"->"保存过程尚未确认完成"
  else->"电脑保存失败"
 }
 fun warning(count:Int,overflow:Boolean,projectionError:Boolean,stale:Boolean,noMonitor:Boolean,inputFailures:Int=0):String?=when {
  overflow->"回复保存失败记录已满，可能有未保存正文，请在电脑检查。"
  count>0->"$count 条最终回复尚未确认保存，提示不代表正文已保存。"
  inputFailures>0->"$inputFailures 轮用户消息尚未同步；已有助手回复仍可读取。"
  projectionError->"电脑回复保存或同步状态不可用；已有内容仍可读取。"
  stale||noMonitor->"电脑回复保存状态尚未确认；已有内容仍可读取。"
  else->null
 }
}
