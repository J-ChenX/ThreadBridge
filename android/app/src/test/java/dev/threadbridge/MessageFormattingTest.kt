package dev.threadbridge

import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.font.FontWeight
import org.commonmark.node.*
import org.commonmark.ext.gfm.tables.TableBlock
import org.junit.Assert.*
import org.junit.Test

class MessageFormattingTest {
 private fun content(text:String)=MessageFormatting.inline(MessageFormatting.parse(text),SpanStyle(),SpanStyle())
 @Test fun screenshotMarkupIsRenderedWithoutLosingContent() {
  val formatted=content("核实了，**当前版本**。设置为 `true`，见[官方说明](https://example.com/docs)。")
  assertEquals("核实了，当前版本。设置为 true，见官方说明。",formatted.text)
  assertTrue(formatted.spanStyles.any{it.item.fontWeight==FontWeight.SemiBold})
  assertEquals(1,formatted.getLinkAnnotations(0,formatted.length).size)
 }
 @Test fun literalEscapesAndCodeAreNotReinterpreted() {
  assertEquals("**原文** 和 a*b",content("\\*\\*原文\\*\\* 和 `a*b`").text)
  val code=MessageFormatting.parse("```kotlin\nval value = \"**literal**\"\n```").firstChild as FencedCodeBlock
  assertEquals("val value = \"**literal**\"\n",code.literal)
 }
 @Test fun unsafeOrLocalLinksStayTextAndNeverBecomeLaunchable() {
  for(link in listOf("javascript:alert(1)","file:///etc/passwd","intent://settings","/home/example.apk")){
   assertFalse(MessageFormatting.webLink(link))
   val formatted=content("[链接]($link)")
   assertTrue(formatted.getLinkAnnotations(0,formatted.length).isEmpty())
   assertTrue(formatted.text.contains("链接"))
  }
  assertTrue(MessageFormatting.webLink("https://example.com/docs"))
 }
 @Test fun listsTablesAndDeepNestingPreserveHumanText() {
  val document=MessageFormatting.parse("3. 第一项\n4. 第二项\n\n| 名称 | 状态 |\n| --- | --- |\n| 电脑 | 在线 |")
  assertTrue(document.firstChild is OrderedList);assertTrue(document.lastChild is TableBlock)
  assertEquals("完整正文",content("*".repeat(100)+"完整正文"+"*".repeat(100)).text)
 }
}
