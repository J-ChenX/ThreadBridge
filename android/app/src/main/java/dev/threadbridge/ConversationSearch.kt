package dev.threadbridge

import java.util.Locale

// Same width folding and per-code-point lowercase as Store::normalize_search.
private fun normalizeTitle(value:String)=buildString {
 value.codePoints().forEach { cp->
  val folded=when(cp){in 0xff01..0xff5e->cp-0xfee0;0x3000->32;else->cp}
  append(String(Character.toChars(folded)).lowercase(Locale.ROOT))
 }
}
internal fun titleMatches(title:String,query:String):Boolean {
 val needle=normalizeTitle(query).filterNot{it.isWhitespace()}
 if(needle.isEmpty())return true
 val haystack=normalizeTitle(title)
 var next=0
 for(ch in haystack){if(ch==needle[next]){next++;if(next==needle.length)return true}}
 return false
}
internal data class ConversationSearchState(val rows:List<ThreadRow> = emptyList(),val loading:Boolean=false,val offline:Boolean=false,val more:Boolean=false)
