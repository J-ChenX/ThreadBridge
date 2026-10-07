package dev.threadbridge

import java.net.ConnectException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import javax.net.ssl.SSLException
import org.junit.Assert.*
import org.junit.Test

class ConnectionDiagnosticsTest {
 @Test fun expiredEndpointAndCredentialsHaveDifferentRecoveryActions(){
  assertTrue(connectionFailure(ApiException(404,"<html>secret</html>")).contains("地址已失效"))
  assertTrue(connectionFailure(ApiException(401,"secret")).contains("重新配对"))
  assertFalse(connectionFailure(ApiException(404,"secret")).contains("重新配对"))
 }
 @Test fun serverPayloadsAndExceptionTextAreNeverRendered(){
  for(error in listOf(ApiException(403,"Bearer private-token"),ApiException(503,"private-token"),Exception("private-token")))
   assertFalse(connectionFailure(error).contains("private-token"))
 }
 @Test fun networkAndInvalidDataFailuresProvideUsefulDiagnostics(){
  assertTrue(connectionFailure(UnknownHostException()).contains("域名"))
  assertTrue(connectionFailure(SocketTimeoutException()).contains("超时"))
  assertTrue(connectionFailure(ConnectException()).contains("无法连接"))
  assertTrue(connectionFailure(SSLException("secret")).contains("安全连接"))
  assertTrue(connectionFailure(InvalidApiResponse()).contains("有效数据"))
  assertTrue(connectionFailure(ResponseTooLarge()).contains("过大"))
 }
}
