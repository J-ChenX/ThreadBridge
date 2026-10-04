package dev.threadbridge

import org.junit.Assert.*
import org.junit.Test

class ConversationAttachmentsTest {
 @Test fun onlyLocalImageMarkersBecomeAttachments() {
  val id="a".repeat(64)
  val body="这是用户问题\n![图片](threadbridge-image:$id)\n"
  assertEquals(listOf(id),ConversationAttachments.images(body))
  assertEquals("这是用户问题",ConversationAttachments.caption(body))
  assertTrue(ConversationAttachments.images("![图片](https://example.test/image.png)").isEmpty())
  assertTrue(ConversationAttachments.images("正文中的 ![图片](threadbridge-image:$id)").isEmpty())
 }
 @Test fun pureImageAndRepeatedAttachment() {
  val marker="![图片](threadbridge-image:${"b".repeat(64)})"
  assertEquals("",ConversationAttachments.caption("$marker\n$marker"))
  assertEquals(1,ConversationAttachments.images("$marker\n$marker").size)
 }
}
