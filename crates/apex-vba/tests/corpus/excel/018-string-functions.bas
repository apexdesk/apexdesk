' corpus-source: generated
' corpus-flagged: no
' corpus-prints: HELLO WORLD
' corpus-prints: hello world
' corpus-prints: Hello
' corpus-prints: 11
' corpus-prints: World
' corpus-prints: 8
Sub Main()
    Dim s As String
    s = "Hello World"
    Debug.Print UCase(s)
    Debug.Print LCase(s)
    Debug.Print Left(s, 5)
    Debug.Print Len(s)
    Debug.Print Right(s, 5)
    Debug.Print InStr(s, "W") + 1
End Sub
