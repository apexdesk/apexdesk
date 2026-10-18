' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 120
' corpus-prints: 55
Function Fact(n As Long) As Long
    If n <= 1 Then Fact = 1 Else Fact = n * Fact(n - 1)
End Function
Function Fib(n As Long) As Long
    If n < 2 Then Fib = n Else Fib = Fib(n - 1) + Fib(n - 2)
End Function
Sub Main()
    Debug.Print Fact(5)
    Debug.Print Fib(10)
End Sub
