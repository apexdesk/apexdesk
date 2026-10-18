' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
' corpus-prints: 8
Sub Main()
    Debug.Print InStr(1, "Hello World", "hello", vbTextCompare)
    Debug.Print InStr(1, "Hello World", "W", vbBinaryCompare) + 1
End Sub
