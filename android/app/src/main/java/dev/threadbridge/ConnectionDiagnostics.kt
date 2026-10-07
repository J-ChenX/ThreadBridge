package dev.threadbridge

import java.net.ConnectException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import javax.net.ssl.SSLException

internal class InvalidApiResponse:Exception("服务未返回有效数据")
internal class ResponseTooLarge:Exception("响应超过限制")
/** Never display response bodies, authentication headers or arbitrary exception text. */
internal fun connectionFailure(error:Exception):String=when(error){
 is ApiException->when(error.code){
  401->"凭证已失效，请重新配对"
  403->"服务器拒绝访问（HTTP 403）"
  404->"服务器地址已失效（HTTP 404），请检查公网入口"
  429->"请求过于频繁，请稍后重试"
  in 500..599->"服务器暂时不可用（HTTP ${error.code}）"
  else->"服务器请求失败（HTTP ${error.code}）"
 }
 is UnknownHostException->"无法解析服务器域名，请检查地址和网络"
 is SocketTimeoutException->"连接服务器超时，请检查网络和公网入口"
 is ConnectException->"无法连接服务器，请检查网络和服务状态"
 is SSLException->"HTTPS 证书或安全连接验证失败"
 is InvalidApiResponse->"服务器未返回有效数据，请检查地址和服务版本"
 is ResponseTooLarge->"服务器响应过大，请检查服务版本"
 else->"同步失败，请稍后重试"
}
internal fun connectedLabel(count:Int,polling:Boolean)="已连接 · $count 个对话"+(if(polling)" · HTTP 轮询"else "")
