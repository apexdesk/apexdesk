' corpus-source: generated
' corpus-flagged: no
' corpus-prints: Hello, you
' corpus-prints: Hello, Bob
Function Greet(Optional nm As String = "you") As String
    Greet = "Hello, " & nm
End Function
Sub Main()
    Debug.Print Greet()
    Debug.Print Greet("Bob")
End Sub
