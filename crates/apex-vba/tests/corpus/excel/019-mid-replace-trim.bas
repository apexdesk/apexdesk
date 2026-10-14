' corpus-source: generated
' corpus-flagged: no
' corpus-prints: [abc]
' corpus-prints: b-b
' corpus-prints: ell
Sub Main()
    Debug.Print "[" & Trim("   abc  ") & "]"
    Debug.Print Replace("a-b", "a", "b")
    Debug.Print Mid("Hello", 2, 3)
End Sub
