package dev.threadbridge

import org.json.JSONArray
import org.json.JSONObject

internal data class QuestionOption(val label:String,val description:String)
internal data class QuestionPrompt(val id:String,val kind:String,val title:String,val options:List<QuestionOption>,val status:String,val answer:String?) {
 val canReply get()=kind=="async"&&status=="pending"
}
internal object QuestionPresentation {
 private const val START="<send_user_message_question_reply>"
 private const val END="</send_user_message_question_reply>"
 fun parse(message:MessageRow):QuestionPrompt?=runCatching {
  require(message.role=="question"&&message.characters<=12000)
  val p=JSONObject(message.text)
  require(p.getInt("schema")==1)
  val options=p.getJSONArray("options")
  QuestionPrompt(p.getString("id"),p.getString("kind"),p.getString("title"),(0 until options.length()).map{val o=options.getJSONObject(it);QuestionOption(o.getString("label"),o.getString("description"))},p.getString("status"),if(p.isNull("answer"))null else p.getString("answer"))
 }.getOrNull()
 fun reply(q:QuestionPrompt,answer:String):String {
  require(q.canReply&&answer.isNotBlank()&&answer.toByteArray().size<=8000){"回答最多 8000 字节"}
  val item=JSONObject().put("questionItemId",q.id).put("question",q.title).put("answer",answer)
  return "$START\n${JSONArray().put(item)}\n$END\n".also{require(it.toByteArray().size<=32000){"回答过长，请缩短后重试"}}
 }
 fun visibleReply(text:String):String=runCatching {
  val body=text.trim()
  if(!body.startsWith(START)||!body.endsWith(END))return text
  val rows=JSONArray(body.removePrefix(START).removeSuffix(END).trim())
  (0 until rows.length()).joinToString("\n\n"){val row=rows.getJSONObject(it);"${row.getString("question")}\n\n回答：${row.getString("answer")}"}
 }.getOrDefault(text)
 fun matchesReply(payload:String,id:String):Boolean=runCatching {
  val text=JSONObject(payload).optString("text").trim()
  if(!text.startsWith(START)||!text.endsWith(END))return false
  val rows=JSONArray(text.removePrefix(START).removeSuffix(END).trim())
  (0 until rows.length()).any{rows.getJSONObject(it).optString("questionItemId")==id}
 }.getOrDefault(false)
 fun statusLabel(status:String)=when(status){"answered"->"已回答";"closed"->"已结束";"failed"->"提问未成功";else->"待回答"}
}
