' corpus-source: generated
' corpus-flagged: no
' corpus-input: World
' corpus-prints: Hello, World
Sub Main()
    Dim nm As String
    nm = InputBox("Your name?", "Greeting")
    Range("A1").Value = "Hello, " & nm
    Debug.Print Range("A1").Value
End Sub
