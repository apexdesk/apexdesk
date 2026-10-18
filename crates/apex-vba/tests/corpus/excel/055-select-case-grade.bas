' corpus-source: generated
' corpus-flagged: no
' corpus-prints: A
' corpus-prints: C
' corpus-prints: F
Function Grade(s As Long) As String
    Select Case s
        Case Is >= 90
            Grade = "A"
        Case 80 To 89
            Grade = "B"
        Case 70 To 79
            Grade = "C"
        Case Else
            Grade = "F"
    End Select
End Function
Sub Main()
    Debug.Print Grade(95)
    Debug.Print Grade(75)
    Debug.Print Grade(20)
End Sub
