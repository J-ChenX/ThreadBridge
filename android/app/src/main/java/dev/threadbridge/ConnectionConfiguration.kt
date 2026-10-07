package dev.threadbridge

import java.net.URI

/** User-entered endpoints and credentials are validated before replacing saved state. */
internal object ConnectionConfiguration {
 fun server(value:String,lan:Boolean):String {
  val uri=try{URI(value.trim())}catch(_:Exception){throw IllegalArgumentException("请输入有效的服务器地址")}
  require(uri.userInfo==null&&uri.query==null&&uri.fragment==null&&uri.host!=null&&uri.port in -1..65535&&uri.port!=0){"请输入有效的服务器地址"}
  val host=uri.host.lowercase()
  val octets=host.split('.').map{it.toIntOrNull()}
  val privateIp=host in setOf("localhost","127.0.0.1")||(octets.size==4&&octets.all{it!=null&&it in 0..255}&&
   (octets[0]==10||(octets[0]==192&&octets[1]==168)||(octets[0]==172&&octets[1] in 16..31)))
  require(uri.scheme=="https"||(lan&&uri.scheme=="http"&&privateIp)){"正式连接需要 HTTPS；局域网测试仅允许私人 IP"}
  return value.trim().trimEnd('/')
 }
 fun key(value:String,current:String):String {
  val key=value.trim().removePrefix("Bearer ").trim().ifEmpty{current}
  require(key.isNotEmpty()){ "请输入连接密钥" }
  require(key.length<=4096&&key.all{it.code in 33..126}){ "连接密钥格式不正确" }
  return key
 }
 fun sameCollection(saved:String,incoming:String):Boolean=incoming.isNotBlank()&&(saved.isEmpty()||saved==incoming)
}
