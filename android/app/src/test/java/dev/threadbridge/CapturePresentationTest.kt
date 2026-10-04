package dev.threadbridge
import org.junit.Assert.*
import org.junit.Test
class CapturePresentationTest {
 @Test fun failuresCannotBeReportedAsSaved(){assertTrue(CapturePresentation.warning(2,false,false,false,false)!!.contains("不代表正文已保存"));assertTrue(CapturePresentation.reason("reply_too_large").contains("未截断"))}
 @Test fun missingAndStaleMonitoringAreVisible(){assertNotNull(CapturePresentation.warning(0,false,false,true,false));assertNotNull(CapturePresentation.warning(0,false,false,false,true));assertNotNull(CapturePresentation.warning(0,false,true,false,false));assertNotNull(CapturePresentation.warning(0,true,false,false,false))}
 @Test fun inputFailureDoesNotClaimTheSavedReplyWasLost(){assertTrue(CapturePresentation.warning(0,false,false,false,false,1)!!.contains("用户消息尚未同步"));assertTrue(CapturePresentation.reason("user_input_capture_failed").contains("助手回复已保存"))}
 @Test fun onlyConfirmedHealthClearsWarning(){assertNull(CapturePresentation.warning(0,false,false,false,false))}
}
